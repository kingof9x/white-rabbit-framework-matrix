//! The control channel between the running monitor and the White Rabbit window: a port of
//! `led_mon/control.py`, so the unchanged Python window can drive the Rust monitor.
//!
//! The monitor listens on a named pipe (Windows) or a Unix socket (Linux) that only this user can
//! reach. Both ends prove they know the key in control.key with the handshake of Python's
//! `multiprocessing.connection` (`wire.rs`). Then the window sends one JSON object and gets one JSON
//! object back with "ok"; one request per connection.

pub mod bridge;
pub mod wire;

#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub mod winserial;

use std::io;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use wire::MessageStream;
use wr_core::{error, info, warn};

/// The longest request taken, as in Python.
const MAX_REQUEST: usize = 16 * 1024 * 1024;

/// Answers one request. Err(text) becomes {"ok": false, "error": text}.
pub trait Handler: Send + Sync {
    fn handle(&self, message: &Map<String, Value>) -> Result<Map<String, Value>, String>;
}

/// Python's `_user_tag`: the user name with only letters, digits, - and _.
fn user_tag() -> String {
    // getpass.getuser() looks at these in this order
    let name = ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "user".into());
    let tag: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    if tag.is_empty() {
        "user".into()
    } else {
        tag
    }
}

/// Where the monitor listens, the same name as the Python monitor's.
pub fn address() -> String {
    if cfg!(windows) {
        format!(r"\\.\pipe\{}-{}", crate::paths::APP_NAME, user_tag())
    } else {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        dir.join(format!("{}-{}.sock", crate::paths::APP_NAME, user_tag())).display().to_string()
    }
}

/// A new random key in control.key, which the window reads to connect.
pub fn new_key() -> io::Result<Vec<u8>> {
    let key = wire::random_bytes(32)?;
    let path = crate::paths::control_key_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    // Readable only by this user on Linux; %LOCALAPPDATA% is already per-user on Windows
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(&mut options.open(&path)?, &key)?;
    Ok(key)
}

/// The reply to one request (already past the handshake), as Python's `serve` makes it.
pub fn reply_to(handler: &dyn Handler, request: &[u8]) -> Value {
    let message = match serde_json::from_slice::<Value>(request) {
        Ok(Value::Object(m)) => m,
        Ok(_) => return json!({"ok": false, "error": "the request isn't a JSON object"}),
        Err(e) => return json!({"ok": false, "error": format!("the request isn't JSON: {e}")}),
    };
    match handler.handle(&message) {
        Ok(mut reply) => {
            reply.entry("ok").or_insert(Value::Bool(true));
            Value::Object(reply)
        }
        Err(e) => {
            let cmd = message.get("cmd").and_then(Value::as_str).unwrap_or("?");
            error!("Error handling control request {cmd}: {e}");
            json!({"ok": false, "error": e})
        }
    }
}

/// One connection: the handshake, one request, one reply.
pub fn serve_connection(conn: &mut dyn MessageStream, key: &[u8], handler: &dyn Handler) -> io::Result<()> {
    wire::server_handshake(conn, key)?;
    let request = conn.recv_message(MAX_REQUEST)?;
    let reply = reply_to(handler, &request);
    conn.send_message(reply.to_string().as_bytes())
}

/// Starts the thread that answers the window. False when the channel can't be set up (logged).
pub fn serve(handler: std::sync::Arc<dyn Handler>) -> bool {
    let key = match new_key() {
        Ok(k) => k,
        Err(e) => {
            warn!("Control channel not available: can't write the key ({e})");
            return false;
        }
    };
    let addr = address();
    #[cfg(windows)]
    let started = pipe::serve(&addr, key, handler);
    #[cfg(unix)]
    let started = unix::serve(&addr, key, handler);
    match started {
        Ok(()) => {
            info!("Control channel listening on {addr}");
            true
        }
        Err(e) => {
            warn!("Control channel not available: {e}");
            false
        }
    }
}

/// A Unix socket, as the Python monitor uses on Linux. The Rust monitor runs on Windows only for now;
/// this is here for the tests, which run a real Python client against it.
#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;

    pub fn listen(addr: &str) -> io::Result<UnixListener> {
        let _ = std::fs::remove_file(addr);
        let listener = UnixListener::bind(addr)?;
        std::fs::set_permissions(addr, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        Ok(listener)
    }

    pub fn run(listener: UnixListener, key: Vec<u8>, handler: Arc<dyn Handler>) {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            if let Err(e) = serve_connection(&mut wire::Framed(stream), &key, handler.as_ref()) {
                info!("Control connection error: {e}");
            }
        }
    }

    pub fn serve(addr: &str, key: Vec<u8>, handler: Arc<dyn Handler>) -> io::Result<()> {
        let listener = listen(addr)?;
        std::thread::Builder::new().name("control".into()).spawn(move || run(listener, key, handler))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl Handler for Echo {
        fn handle(&self, m: &Map<String, Value>) -> Result<Map<String, Value>, String> {
            match m.get("cmd").and_then(Value::as_str) {
                Some("fail") => Err("it broke".into()),
                Some("refuse") => Ok(json!({"ok": false, "error": "no"}).as_object().unwrap().clone()),
                _ => Ok(m.clone()),
            }
        }
    }

    #[test]
    fn replies_get_ok_unless_they_say_otherwise() {
        assert_eq!(reply_to(&Echo, br#"{"cmd": "x", "n": 1}"#), json!({"cmd": "x", "n": 1, "ok": true}));
        assert_eq!(reply_to(&Echo, br#"{"cmd": "refuse"}"#), json!({"ok": false, "error": "no"}));
        assert_eq!(reply_to(&Echo, br#"{"cmd": "fail"}"#), json!({"ok": false, "error": "it broke"}));
        assert_eq!(reply_to(&Echo, b"[1]")["ok"], json!(false));
    }

    #[test]
    fn the_address_matches_python() {
        let a = address();
        assert!(a.contains("WhiteRabbit-"), "{a}");
    }

    /// The real Python client from led_mon/control.py's side (multiprocessing.connection.Client)
    /// against this server. Skipped when no Python 3 is around; set WR_TEST_PYTHON to pick one.
    #[cfg(unix)]
    #[test]
    fn a_real_python_client_connects() {
        let python = std::env::var("WR_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
        if std::process::Command::new(&python).arg("-V").output().is_err() {
            eprintln!("no {python}; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("wr-control-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let addr = dir.join("c.sock").display().to_string();
        let key = wire::random_bytes(32).unwrap();
        let listener = unix::listen(&addr).unwrap();
        let k = key.clone();
        std::thread::spawn(move || unix::run(listener, k, std::sync::Arc::new(Echo)));
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let script = r##"
import json, sys
from multiprocessing.connection import Client
from multiprocessing import AuthenticationError
addr, key = sys.argv[1], bytes.fromhex(sys.argv[2])
c = Client(addr, authkey=key)
c.send_bytes(json.dumps({"cmd": "status", "big": "x" * 70000}).encode())
reply = json.loads(c.recv_bytes().decode())
c.close()
assert reply["ok"] is True and reply["cmd"] == "status" and len(reply["big"]) == 70000, reply
try:
    Client(addr, authkey=b"wrong key")
    raise SystemExit("a wrong key got in")
except AuthenticationError:
    pass
# Python 3.11 and older: a bare 20-byte challenge, bare HMAC-MD5 answers
import hmac, os
from multiprocessing import connection as mc
c = mc.SocketClient(addr)
challenge = c.recv_bytes(256)
assert challenge.startswith(b"#CHALLENGE#{sha256}"), challenge
c.send_bytes(hmac.new(key, challenge[len(b"#CHALLENGE#"):], "md5").digest())
assert c.recv_bytes(256) == b"#WELCOME#"
message = os.urandom(20)
c.send_bytes(b"#CHALLENGE#" + message)
assert c.recv_bytes(256) == hmac.new(key, message, "md5").digest()
c.send_bytes(b"#WELCOME#")
c.send_bytes(b'{"cmd": "old"}')
assert json.loads(c.recv_bytes())["cmd"] == "old"
print("python client ok")
"##;
        let out = std::process::Command::new(&python).args(["-c", script, &addr, &hex]).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
