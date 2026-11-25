//! Talking to the running monitor (WhiteRabbit.exe) over its control channel: a named pipe on Windows,
//! a Unix socket elsewhere, with the handshake of Python's `multiprocessing.connection` (the monitor's
//! `control/wire.rs`, used here as it is). One JSON request and one JSON reply per connection.
//!
//! While the monitor runs it has the panels' serial ports open, and Windows lets only one program open a
//! port, so the window sends its panel commands through the monitor ("serial", "serial_many") and
//! borrows panels with "hold" / "release" (`white_rabbit/bridge.py`).
//!
//! # How one request goes
//!
//! 1. Read the shared secret key from the control key file (`paths::control_key_file`). The monitor writes
//!    that file when it starts; if it is missing there is no monitor and the request fails quietly.
//! 2. Connect to the pipe / socket named by `address()` (one per Windows user, so two users on one PC
//!    don't talk to each other's monitor).
//! 3. Handshake, both directions: the monitor sends `#CHALLENGE#{sha256}` plus random bytes, we answer
//!    with an HMAC of them made with the key, and it says `#WELCOME#`; then we challenge it the same way.
//!    That proves both ends know the key without ever sending it, so another program on the PC can't
//!    drive the panels through the monitor.
//! 4. Send the request as one JSON message, read one JSON reply, close the connection.
//!
//! Who uses it: `panels.rs` (`Sender`, `Lender`) for panel traffic, `app.rs` for the status / music /
//! identify requests, and pages for their own commands. Every call blocks, so it runs on a worker
//! thread, never on the UI thread (`request` also has its own timeout thread, below).

use std::io;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

// The monitor's own message framing and handshake code, compiled straight into this crate from its
// source file so both ends are guaranteed to speak the same protocol. Parts the window doesn't use
// (the server side) would otherwise warn as unused.
#[allow(dead_code)]
#[path = "../../wr-monitor/src/control/wire.rs"]
mod wire;

// `MessageStream`: send one whole message / receive one whole message, whatever the transport is.
use wire::MessageStream;

/// Longest wait for a panel's reply. Old firmware doesn't answer some commands at all.
///
/// In seconds. When the window opens the ports itself, it also caps any timeout passed to `Sender::send`.
pub const MAX_READ_SECONDS: f64 = 3.0;
/// Biggest reply accepted from the monitor, in bytes (16 MiB). A longer one is treated as broken data
/// rather than read into memory without limit.
const MAX_REPLY: usize = 16 * 1024 * 1024;

/// The current user's login name, cut down to letters, digits, `-` and `_`, for the pipe name.
/// Checks the same environment variables, in the same order, as the monitor's `control` module (and
/// Python's `getpass.getuser()`), so both ends build the same pipe name. Falls back to "user".
fn user_tag() -> String {
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

///
/// Windows: `\\.\pipe\WhiteRabbit-<user>`. Elsewhere: `WhiteRabbit-<user>.sock` in `$XDG_RUNTIME_DIR`
/// (or the temp folder when that isn't set).
/// Where the monitor listens (the same name as `control::address` in the monitor).
pub fn address() -> String {
    if cfg!(windows) {
        format!(r"\\.\pipe\WhiteRabbit-{}", user_tag())
    } else {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        dir.join(format!("WhiteRabbit-{}.sock", user_tag())).display().to_string()
    }
}

/// `Client(address, authkey=key)`: answer the monitor's challenge, then challenge it.
///
/// Both halves must pass: an error from either means the key didn't match (or the connection broke),
/// and the request is abandoned.
fn client_handshake(conn: &mut dyn MessageStream, key: &[u8]) -> io::Result<()> {
    wire::answer_challenge(conn, key)?;
    wire::deliver_challenge(conn, key)
}

/// The Windows named-pipe client: just enough of the Win32 file API to open the monitor's pipe and send
/// and receive whole messages on it.
#[cfg(windows)]
mod pipe {
    use super::*;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_MORE_DATA, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING};
    use windows_sys::Win32::System::Pipes::{SetNamedPipeHandleState, WaitNamedPipeW, PIPE_READMODE_MESSAGE};

    /// An open pipe handle. Closed when dropped.
    pub struct Pipe(HANDLE);
    // SAFETY: a pipe handle can be used from any thread; only one thread uses this one at a time.
    unsafe impl Send for Pipe {}

    impl Drop for Pipe {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and still open.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// A Rust string as the null-terminated UTF-16 text the Win32 "W" functions take.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Opens the monitor's pipe for reading and writing. When every pipe instance is busy (another
    /// request is being answered) it waits up to a second for one to free up and tries again, five times
    /// in all. Any other error (most often "file not found": the monitor isn't running) is returned at once.
    pub fn connect(addr: &str) -> io::Result<Pipe> {
        let name = wide(addr);
        for _ in 0..5 {
            // SAFETY: name is a null-terminated wide string that outlives the call.
            let handle = unsafe {
                CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, null_mut(), OPEN_EXISTING, 0, null_mut())
            };
            if handle != INVALID_HANDLE_VALUE {
                // The monitor's pipe is a message pipe: switch our end to message mode so each ReadFile returns
                // (part of) exactly one message, which is how `recv_message` knows where a message ends.
                let mode = PIPE_READMODE_MESSAGE;
                // SAFETY: the handle is a pipe we just opened; mode lives across the call.
                unsafe { SetNamedPipeHandleState(handle, &mode, null_mut(), null_mut()) };
                return Ok(Pipe(handle));
            }
            // SAFETY: plain call, right after CreateFileW.
            let err = unsafe { GetLastError() };
            if err != ERROR_PIPE_BUSY {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            // SAFETY: name is null-terminated.
            // Waits up to 1000 ms for an instance to become free; its result doesn't matter, the loop tries again.
            unsafe { WaitNamedPipeW(name.as_ptr(), 1000) };
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "the monitor's pipe stayed busy"))
    }

    impl MessageStream for Pipe {
        /// Writes one message. In message mode a single WriteFile is one message, so the bytes go in one call.
        fn send_message(&mut self, data: &[u8]) -> io::Result<()> {
            let mut written = 0u32;
            // SAFETY: the buffer and its length are passed together.
            let ok = unsafe { WriteFile(self.0, data.as_ptr(), data.len() as u32, &mut written, null_mut()) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// Reads one whole message, in 8 KiB pieces: ReadFile says ERROR_MORE_DATA while the message has more
        /// to come, and succeeds on its last piece. Fails when the total passes `max` bytes.
        fn recv_message(&mut self, max: usize) -> io::Result<Vec<u8>> {
            let mut message = Vec::new();
            let mut buf = vec![0u8; 8192];
            loop {
                let mut read = 0u32;
                // SAFETY: the buffer and its length are passed together.
                let ok = unsafe { ReadFile(self.0, buf.as_mut_ptr(), buf.len() as u32, &mut read, null_mut()) };
                // SAFETY: plain call, right after ReadFile.
                let err = if ok == 0 { unsafe { GetLastError() } } else { 0 };
                message.extend_from_slice(&buf[..read as usize]);
                if message.len() > max {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "the reply is too long"));
                }
                match err {
                    0 => return Ok(message),
                    ERROR_MORE_DATA => continue,
                    e => return Err(io::Error::from_raw_os_error(e as i32)),
                }
            }
        }
    }
}

/// The shared secret for the handshake, as written by the monitor. An error here (usually NotFound)
/// means no monitor has started for this user.
fn read_key() -> io::Result<Vec<u8>> {
    std::fs::read(crate::paths::control_key_file())
}

/// One complete round trip (key, connect, handshake, request, reply) with no timeout of its own: this
/// blocks until the monitor answers or the connection breaks. Use `request`, which adds the timeout.
fn exchange(message: &Value) -> io::Result<Value> {
    let key = read_key()?;
    let addr = address();
    #[cfg(windows)]
    let mut conn = pipe::connect(&addr)?;
    #[cfg(unix)]
    let mut conn = wire::Framed(std::os::unix::net::UnixStream::connect(&addr)?);
    client_handshake(&mut conn, &key)?;
    conn.send_message(message.to_string().as_bytes())?;
    let reply = conn.recv_message(MAX_REPLY)?;
    serde_json::from_slice(&reply).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Sends one request to the monitor. None when no monitor is running or it didn't answer in time.
///
/// The exchange runs on a throwaway thread and this waits for it at most `timeout`. If time runs out
/// that thread is simply left behind; it ends by itself when the pipe closes, and its late answer is
/// dropped because nobody is receiving on the channel any more. "Not found" errors (no monitor) are
/// expected and not logged; other errors go to the log at info level.
pub fn request(message: Value, timeout: Duration) -> Option<Value> {
    let (tx, rx) = mpsc::channel();
    // A stale pipe must not hang the window, and the handshake has no timeout of its own
    std::thread::spawn(move || {
        let _ = tx.send(exchange(&message));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(reply)) => Some(reply),
        Ok(Err(e)) => {
            if e.kind() != io::ErrorKind::NotFound {
                wr_core::info!("Control request failed: {e}");
            }
            None
        }
        Err(_) => None,
    }
}

/// Remembers for a moment whether the monitor is running, so each command doesn't have to ask.
///
/// Holds the time of the last status check and its answer, and reuses that answer for `ttl` (2 s).
/// Shared between threads behind an `Arc` in `app::Ctx`.
pub struct Link {
    /// (when the status was last asked, the monitor's status reply or None if it wasn't running)
    state: Mutex<(Option<Instant>, Option<Value>)>,
    /// How long a status answer is trusted before asking again.
    ttl: Duration,
}

impl Default for Link {
    fn default() -> Self {
        Link { state: Mutex::new((None, None)), ttl: Duration::from_secs(2) }
    }
}

impl Link {
    /// The monitor's status ({"ports": [...], ...}), or None if it isn't running.
    ///
    /// `fresh` forces a new request; otherwise a cached answer under 2 s old is reused. The lock is held
    /// during the request (up to 1.5 s), so two threads asking at once make only one request. A reply
    /// without `"ok": true` counts as not running.
    pub fn status(&self, fresh: bool) -> Option<Value> {
        let mut state = self.state.lock().unwrap();
        let stale = state.0.is_none_or(|t| t.elapsed() > self.ttl);
        if fresh || stale {
            let reply = request(json!({"cmd": "status"}), Duration::from_millis(1500));
            state.1 = reply.filter(|r| r.get("ok").and_then(Value::as_bool) == Some(true));
            state.0 = Some(Instant::now());
        }
        state.1.clone()
    }

    /// True when the monitor answered a status request (see `status` for `fresh`).
    pub fn running(&self, fresh: bool) -> bool {
        self.status(fresh).is_some()
    }

    /// Throws away the cached status, so the next `running(false)` asks the monitor again.
    fn forget(&self) {
        self.state.lock().unwrap().0 = None;
    }

    /// A request that must succeed: Err(text) when the monitor didn't answer or said no.
    ///
    /// Waits up to 5 s (the longest panel read plus 2 s for the monitor's own work). When the monitor
    /// doesn't answer at all, the cached status is forgotten so callers notice it has gone.
    pub fn call(&self, message: Value) -> Result<Map<String, Value>, String> {
        let timeout = Duration::from_secs_f64(MAX_READ_SECONDS + 2.0);
        match request(message, timeout) {
            None => {
                self.forget();
                Err("The White Rabbit monitor stopped answering".into())
            }
            Some(Value::Object(reply)) => {
                if reply.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(reply)
                } else {
                    Err(reply
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("The monitor couldn't reach the panel")
                        .to_string())
                }
            }
            Some(_) => Err("The monitor's reply wasn't understood".into()),
        }
    }

    /// Borrows the panels from the monitor. Returns the monitor's last frame on each one it had drawn,
    /// by port, for the app transition.
    ///
    /// Sends this window's process id with the request: if the window dies without releasing them, the
    /// monitor sees that process is gone and takes the panels back by itself. Does nothing (and succeeds)
    /// when no ports are given or no monitor is running. Frames come as hex text, one byte per LED,
    /// column by column (WIDTH columns of HEIGHT LEDs); one of the wrong length is skipped.
    pub fn hold(&self, ports: &[String]) -> Result<Vec<(String, crate::lead_in::Frame)>, String> {
        let mut frames = Vec::new();
        if !ports.is_empty() && self.running(false) {
            let reply = self.call(json!({"cmd": "hold", "ports": ports, "pid": std::process::id()}))?;
            if let Some(Value::Object(map)) = reply.get("frames") {
                for (port, hex) in map {
                    let bytes = unhex(hex.as_str().unwrap_or(""));
                    if bytes.len() == wr_core::WIDTH * wr_core::HEIGHT {
                        let mut frame = [[0u8; wr_core::HEIGHT]; wr_core::WIDTH];
                        for (x, col) in frame.iter_mut().enumerate() {
                            // Column x is bytes x*HEIGHT .. (x+1)*HEIGHT: the frame is stored column-major,
                            // frame[x][y].
                            col.copy_from_slice(&bytes[x * wr_core::HEIGHT..(x + 1) * wr_core::HEIGHT]);
                        }
                        frames.push((port.clone(), frame));
                    }
                }
            }
        }
        Ok(frames)
    }

    /// Lets the monitor draw on these ports again (None: every port this window holds). Succeeds without
    /// asking when no monitor is running, since there is nothing to give back to.
    pub fn release(&self, ports: Option<&[String]>) -> Result<(), String> {
        if self.running(false) {
            self.call(json!({"cmd": "release", "ports": ports}))?;
        }
        Ok(())
    }
}

/// Bytes as the hex text the control channel carries them in.
///
/// Two lowercase hex digits per byte, because JSON has no way to carry raw bytes.
pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// The reverse of `hex`. A pair that isn't valid hex is skipped and an odd last digit ignored, rather
/// than failing the whole reply.
pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).filter_map(|i| u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()).collect()
}

/// Tests for the hex encoding and the client side of the control channel.
#[cfg(test)]
mod tests {
    use super::*;

    /// `hex` and `unhex` turn bytes into text and back unchanged.
    #[test]
    fn hex_round_trip() {
        let data = [0x32, 0xac, 0x00, 0xff];
        assert_eq!(hex(&data), "32ac00ff");
        assert_eq!(unhex("32ac00ff"), data);
    }

    /// A full request against a pretend monitor: address, key file, both handshake halves and one JSON
    /// request / reply all work together.
    #[test]
    #[cfg(unix)]
    fn talks_to_a_monitor_on_a_unix_socket() {
        // A pretend monitor: the real handshake, then it echoes the command name back
        let dir = std::env::temp_dir().join(format!("wr-window-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        std::env::set_var("WHITE_RABBIT_LOG_DIR", &dir);
        let key = b"0123456789abcdef0123456789abcdef".to_vec();
        std::fs::write(crate::paths::control_key_file(), &key).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(address()).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut conn = wire::Framed(stream);
            wire::server_handshake(&mut conn, &key).unwrap();
            let request: Value = serde_json::from_slice(&conn.recv_message(1 << 20).unwrap()).unwrap();
            let reply = json!({"ok": true, "you_said": request["cmd"]});
            conn.send_message(reply.to_string().as_bytes()).unwrap();
        });
        let reply = request(json!({"cmd": "status"}), Duration::from_secs(5)).unwrap();
        assert_eq!(reply["you_said"], "status");
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
