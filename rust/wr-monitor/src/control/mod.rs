//! The control channel between the running monitor and the White Rabbit window: a port of
//! `led_mon/control.py`. It was first written so the Python window could drive the Rust monitor; the
//! window is Rust too now (`wr-window/src/link.rs`), and the protocol was kept so either window works
//! with either monitor, and the Python client test below still checks it byte for byte.
//!
//! The monitor listens on a named pipe (Windows) or a Unix socket (Linux) that only this user can
//! reach. Both ends prove they know the key in control.key with the handshake of Python's
//! `multiprocessing.connection` (`wire.rs`). Then the window sends one JSON object and gets one JSON
//! object back with "ok"; one request per connection.

/// The window's direct panel access: raw serial commands, and holding/releasing panels.
pub mod bridge;
/// What each request ("cmd") does.
pub mod commands;
/// Python's message framing and the key handshake.
pub mod wire;

/// The Windows named pipe server.
#[cfg(windows)]
pub mod pipe;

use std::io;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use wire::MessageStream;
use wr_core::{error, info, warn};

/// The longest request taken, as in Python.
/// 16 MB; a bigger request is refused before it is read into memory.
const MAX_REQUEST: usize = 16 * 1024 * 1024;

/// Answers one request. Err(text) becomes {"ok": false, "error": text}.
pub trait Handler: Send + Sync {
    /// Called on the control thread for each request that passed the handshake. `message` is the
    /// request's JSON object (with "cmd" naming the command).
    fn handle(&self, message: &Map<String, Value>) -> Result<Map<String, Value>, String>;
}

/// Python's `_user_tag`: the user name with only letters, digits, - and _.
///
/// Used in the pipe/socket name so two Windows users on one PC each get their own channel.
fn user_tag() -> String {
    // getpass.getuser() looks at these in this order
    let name = ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "user".into());
    // Drop anything that could break a pipe or file name (spaces, slashes, dots ...).
    let tag: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    if tag.is_empty() {
        "user".into()
    } else {
        tag
    }
}

/// Where the monitor listens, the same name as the Python monitor's.
///
/// Windows: `\\.\pipe\WhiteRabbit-<user>`; Linux: `WhiteRabbit-<user>.sock` in XDG_RUNTIME_DIR
/// (or the temp folder).
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
///
/// 32 random bytes, written fresh at every start, so a key from an earlier run is useless. Err if
/// the folder or the file can't be written; the control channel then doesn't start.
pub fn new_key() -> io::Result<Vec<u8>> {
    // A new key on every start: a window must read the current file to get in.
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
///
/// Never fails: a request that isn't a JSON object, or a handler error, becomes `{"ok": false,
/// "error": ...}`. A successful reply gets `"ok": true` unless the handler set "ok" itself.
pub fn reply_to(handler: &dyn Handler, request: &[u8]) -> Value {
    // The request must be a JSON object such as {"cmd": "status"}.
    let message = match serde_json::from_slice::<Value>(request) {
        Ok(Value::Object(m)) => m,
        Ok(_) => return json!({"ok": false, "error": "the request isn't a JSON object"}),
        Err(e) => return json!({"ok": false, "error": format!("the request isn't JSON: {e}")}),
    };
    match handler.handle(&message) {
        Ok(mut reply) => {
            // A handler may set "ok": false itself (a refusal that isn't an error); that is kept.
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
///
/// Err when the handshake fails (wrong key), the request is too big, or the connection breaks;
/// in that case no reply is sent and the caller just logs it and waits for the next client.
pub fn serve_connection(conn: &mut dyn MessageStream, key: &[u8], handler: &dyn Handler) -> io::Result<()> {
    wire::server_handshake(conn, key)?;
    // Only after both sides proved the key is the request read.
    let request = conn.recv_message(MAX_REQUEST)?;
    let reply = reply_to(handler, &request);
    conn.send_message(reply.to_string().as_bytes())
}

/// Starts the thread that answers the window. False when the channel can't be set up (logged).
///
/// Called once from `app::run`. Writes a new key, then starts the `control` thread on the pipe (or
/// socket). Without a control channel the monitor still runs; only the window can't talk to it.
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

    /// Binds the socket (removing a stale one from an earlier run) and makes it readable and writable
    /// by this user only.
    pub fn listen(addr: &str) -> io::Result<UnixListener> {
        let _ = std::fs::remove_file(addr);
        let listener = UnixListener::bind(addr)?;
        std::fs::set_permissions(addr, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        Ok(listener)
    }

    /// Answers clients one at a time, forever. A client gets 10 s per read, so a silent one can't block
    /// the others for long.
    pub fn run(listener: UnixListener, key: Vec<u8>, handler: Arc<dyn Handler>) {
        // One client at a time; a failed accept is skipped.
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            if let Err(e) = serve_connection(&mut wire::Framed(stream), &key, handler.as_ref()) {
                info!("Control connection error: {e}");
            }
        }
    }

    /// Binds the socket here (so errors reach the caller), then answers on the `control` thread.
    pub fn serve(addr: &str, key: Vec<u8>, handler: Arc<dyn Handler>) -> io::Result<()> {
        let listener = listen(addr)?;
        std::thread::Builder::new().name("control".into()).spawn(move || run(listener, key, handler))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handler that echoes the request, or fails / refuses for the "fail" / "refuse" commands.
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

    /// "ok": true is added to plain replies, a handler's own "ok": false is kept, and errors become
    /// "ok": false with the error text.
    #[test]
    fn replies_get_ok_unless_they_say_otherwise() {
        assert_eq!(reply_to(&Echo, br#"{"cmd": "x", "n": 1}"#), json!({"cmd": "x", "n": 1, "ok": true}));
        assert_eq!(reply_to(&Echo, br#"{"cmd": "refuse"}"#), json!({"ok": false, "error": "no"}));
        assert_eq!(reply_to(&Echo, br#"{"cmd": "fail"}"#), json!({"ok": false, "error": "it broke"}));
        assert_eq!(reply_to(&Echo, b"[1]")["ok"], json!(false));
    }

    /// The address has the same "WhiteRabbit-<user>" form as the Python monitor's.
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
        // The script below connects as the window's Python code would, checks a big round trip, a
        // wrong key being refused, and the old (Python 3.11) handshake done by hand.
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
        // The server runs on its own thread until the test process ends.
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

/// Fuzzing the control channel: whatever a client sends, the monitor answers or drops the connection,
/// and never panics (a panic in the control thread would leave the window unable to reach it).
#[cfg(test)]
mod fuzz {
    use super::*;
    use std::collections::VecDeque;

    /// A tiny xorshift random number generator, so the fuzz tests are repeatable without a crate.
    struct Rng(u64);
    impl Rng {
        /// The next pseudo-random number (xorshift64: three shifts and XORs).
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        /// `n` pseudo-random bytes.
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
    }

    /// A client that sends `script` as its messages and records the monitor's replies.
    struct Scripted {
        script: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }

    impl wire::MessageStream for Scripted {
        /// Records what the monitor sends.
        fn send_message(&mut self, data: &[u8]) -> io::Result<()> {
            self.sent.push(data.to_vec());
            Ok(())
        }
        /// Hands out the next scripted message; end of script reads as the client hanging up.
        fn recv_message(&mut self, max: usize) -> io::Result<Vec<u8>> {
            let m = self.script.pop_front().ok_or(io::ErrorKind::UnexpectedEof)?;
            if m.len() > max {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(m)
        }
    }

    /// A handler that echoes every request back.
    struct Echo;
    impl Handler for Echo {
        fn handle(&self, m: &Map<String, Value>) -> Result<Map<String, Value>, String> {
            Ok(m.clone())
        }
    }

    /// 5000 random handshakes: all fail, none panics, and none gets more than the challenge (and
    /// maybe #FAILURE#) back.
    #[test]
    fn garbage_handshakes_are_refused_without_panicking() {
        // Fixed seeds make every run test the same inputs, so a failure can be reproduced.
        let key = b"the key".to_vec();
        let mut rng = Rng(0xA5A5_1234_5678_9ABC);
        for _ in 0..5000 {
            let mut messages = VecDeque::new();
            for _ in 0..rng.next() % 4 {
                let len = (rng.next() % 300) as usize;
                let mut m = rng.bytes(len);
                // Sometimes start like a real answer, so the parser goes further in
                if rng.next().is_multiple_of(3) {
                    m.splice(0..0, b"{sha256}".iter().copied());
                }
                messages.push_back(m);
            }
            // 0-3 random messages: never a valid answer to our random challenge.
            let mut client = Scripted { script: messages, sent: vec![] };
            let result = serve_connection(&mut client, &key, &Echo);
            assert!(result.is_err(), "random bytes passed the handshake");
            // Only the challenge (and maybe #FAILURE#) went out: no reply to an unproven client
            assert!(client.sent.len() <= 2);
        }
    }

    /// 20000 random or damaged requests: each gets a JSON reply with a boolean "ok".
    #[test]
    fn random_requests_get_a_json_reply() {
        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        let samples = [
            r#"{"cmd": "serial", "port": "COM4", "writes": ["32ac"], "read": 32}"#,
            r#"{"cmd": "preview", "frames": {"left": [[0, 255]]}, "seconds": 5}"#,
            r#"{"cmd": "hold", "ports": ["COM4"], "pid": 1}"#,
        ];
        for i in 0..20_000 {
            // Half pure noise, half real-looking requests with a few bytes damaged.
            let request = if i % 2 == 0 {
                let len = (rng.next() % 200) as usize;
                rng.bytes(len)
            } else {
                let mut s = samples[i % 3].as_bytes().to_vec();
                for _ in 0..1 + rng.next() % 5 {
                    let at = (rng.next() as usize) % s.len();
                    s[at] = rng.next() as u8;
                }
                s
            };
            let reply = reply_to(&Echo, &request);
            assert!(reply.get("ok").is_some_and(Value::is_boolean), "{reply}");
        }
    }

    /// Bad length headers give errors rather than huge allocations or hangs.
    #[test]
    fn oversized_and_broken_frames_are_refused() {
        // A 4-byte length saying 2 GB, a negative length, a short body: errors, not allocations or hangs
        for bytes in [&b"\x7f\xff\xff\xff"[..], b"\xff\xff\xff\xfe", b"\x00\x00\x00\x10abc", b"\xff\xff\xff\xff\xff"] {
            let mut framed = wire::Framed(VecDeque::from(bytes.to_vec()));
            assert!(wire::MessageStream::recv_message(&mut framed, MAX_REQUEST).is_err());
        }
    }
}
