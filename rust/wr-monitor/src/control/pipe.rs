//! The control channel's named pipe on Windows, as Python's `multiprocessing.connection.Listener`
//! makes it: message mode (each message is one pipe message, no length before it), and a new pipe
//! instance waiting for the next client while one is answered, so a client never finds no pipe.
//! Unlike Python's, only the current user may open it, and only from this PC.
//!
//! A named pipe is a Windows object with a path like `\\.\pipe\WhiteRabbit-<user>` that a server
//! creates and a client opens like a file. Each `CreateNamedPipeW` makes one "instance" that one
//! client at a time can be connected to. `serve` makes the first instance (failing if another program
//! already owns the name) and starts the `control` thread, which loops forever: wait for a client
//! (`ConnectNamedPipe`), make the next instance, run the handshake and one request, disconnect.
//! Requests are answered one at a time, in order.
//!
//! Reads and writes are blocking. A client that connects and then sends nothing keeps the control
//! thread waiting; only this user can connect, so that can only come from a stuck window.

use std::io::{self, Error, ErrorKind};
use std::ptr::null_mut;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_BROKEN_PIPE, ERROR_MORE_DATA, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
    HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::wire::MessageStream;
use super::{serve_connection, Handler};
use wr_core::{info, warn};
use wr_panel::winports::wide;

/// Size in bytes of the pipe's in and out buffers and of each read; longer messages arrive in parts.
const BUFFER: u32 = 8192;

/// One connected client.
///
/// Holds the pipe instance's handle; closing it is the serving loop's job, not this wrapper's.
struct PipeConn(HANDLE);

impl MessageStream for PipeConn {
    /// Writes the whole message as one pipe message.
    fn send_message(&mut self, data: &[u8]) -> io::Result<()> {
        let mut written = 0u32;
        // SAFETY: the buffer and its length are passed together.
        let ok = unsafe { WriteFile(self.0, data.as_ptr(), data.len() as u32, &mut written, null_mut()) };
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        if written as usize != data.len() {
            return Err(Error::new(ErrorKind::WriteZero, "the message was cut short"));
        }
        Ok(())
    }

    /// Reads one pipe message, in 8 KB parts until it is complete. Err if it grows past `max`, if the
    /// client hung up (UnexpectedEof) or on any other pipe error.
    fn recv_message(&mut self, max: usize) -> io::Result<Vec<u8>> {
        let mut message = Vec::new();
        let mut buf = vec![0u8; BUFFER as usize];
        loop {
            let mut read = 0u32;
            // SAFETY: the buffer and its length are passed together.
            let ok = unsafe { ReadFile(self.0, buf.as_mut_ptr(), buf.len() as u32, &mut read, null_mut()) };
            // SAFETY: plain call, right after ReadFile.
            // In message mode a read that gets only part of a message "fails" with ERROR_MORE_DATA but
            // still fills the buffer; the rest comes with the next read.
            let err = if ok == 0 { unsafe { GetLastError() } } else { 0 };
            message.extend_from_slice(&buf[..read as usize]);
            if message.len() > max {
                return Err(Error::new(ErrorKind::InvalidData, "bad message length"));
            }
            match err {
                0 => return Ok(message),
                // The rest of a long message is still waiting
                ERROR_MORE_DATA => continue,
                ERROR_BROKEN_PIPE => return Err(ErrorKind::UnexpectedEof.into()),
                e => return Err(Error::from_raw_os_error(e as i32)),
            }
        }
    }
}

/// The current user's SID, as text (S-1-5-21-...).
///
/// A SID (security identifier) is Windows' unique id for an account. It is read from this process's
/// access token, so it is whoever runs White Rabbit.
fn user_sid() -> io::Result<String> {
    // SAFETY: plain Win32 calls; the token is closed and the string freed before returning.
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(Error::last_os_error());
        }
        let mut size = 0u32;
        // First call with no buffer only reports the size needed (it "fails" on purpose).
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
        // u64s keep the TOKEN_USER in the buffer aligned
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), size, &mut size);
        CloseHandle(token);
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        // Sound: Windows filled the aligned buffer with a TOKEN_USER (plus the SID it points into).
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text: *mut u16 = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(Error::last_os_error());
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text as HLOCAL);
        Ok(sid)
    }
}

/// Security that lets only the current user open the pipe. Kept for as long as the program runs.
///
/// Without this a named pipe gets a default access list that other local accounts can often read.
fn only_this_user() -> io::Result<SECURITY_ATTRIBUTES> {
    // A protected access list with one entry: full access for this user, nobody else
    // SDDL text: D: = access list, P = protected (no inherited entries), (A;;GA;;;SID) = Allow,
    // Generic All, to that SID.
    let sddl = wide(&format!("D:P(A;;GA;;;{})", user_sid()?));
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: plain call; the descriptor is never freed, as the pipe instances keep using it.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(Error::last_os_error());
    }
    Ok(SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    })
}

/// What is needed to make more instances of the pipe.
struct Pipe {
    /// The pipe's path as UTF-16 with a closing zero.
    name: Vec<u16>,
    /// The "only this user" security, given to every instance.
    security: SECURITY_ATTRIBUTES,
}

// SAFETY: the security descriptor is only read, and lives as long as the program.
unsafe impl Send for Pipe {}

impl Pipe {
    /// Creates one pipe instance waiting for a client. `first` makes creation fail if any instance of
    /// this name exists already (another program squatting on the name), so we never share it.
    fn instance(&self, first: bool) -> io::Result<HANDLE> {
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            // Someone else already serving this name is an error, not a pipe we share
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: plain call; the name and the security outlive it.
        let handle = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open_mode,
                // Message-based in both directions, blocking calls, and no clients from other PCs.
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                // Default client time-out (50 ms) for WaitNamedPipe; the server never waits on it.
                0,
                &self.security,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(Error::last_os_error());
        }
        Ok(handle)
    }

    /// The `control` thread's loop; never returns. `current` is the instance waiting for a client.
    fn run(self, mut current: HANDLE, key: Vec<u8>, handler: Arc<dyn Handler>) {
        loop {
            // SAFETY: plain calls on our own pipe handles.
            unsafe {
                // Blocks until a client opens the pipe.
                let connected = ConnectNamedPipe(current, null_mut()) != 0 || {
                    let err = GetLastError();
                    // Already connected before we waited; or came and went (Python's Issue 14725)
                    err == ERROR_PIPE_CONNECTED || err == ERROR_NO_DATA
                };
                // The next client waits on a new instance while this one is answered
                let next = match self.instance(false) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        warn!("Control channel: no new pipe ({e})");
                        // Wait a little so a persistent failure doesn't turn into a busy loop.
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        None
                    }
                };
                if connected {
                    let mut conn = PipeConn(current);
                    if let Err(e) = serve_connection(&mut conn, &key, handler.as_ref()) {
                        info!("Control connection error: {e}");
                    }
                    // Let the client read the reply before the pipe goes
                    FlushFileBuffers(current);
                }
                // Ends this client's connection (the client's handle then sees a broken pipe).
                DisconnectNamedPipe(current);
                // Without a new instance, this one is used again
                if let Some(h) = next {
                    CloseHandle(current);
                    current = h;
                }
            }
        }
    }
}

/// Starts the thread that answers on the named pipe `addr`.
///
/// The first instance is made here, before the thread starts, so a failure (name already taken,
/// e.g. another monitor) is reported to the caller. Returns at once.
pub fn serve(addr: &str, key: Vec<u8>, handler: Arc<dyn Handler>) -> io::Result<()> {
    let pipe = Pipe { name: wide(addr), security: only_this_user()? };
    let first = pipe.instance(true)?;
    // A raw handle isn't `Send`; passed to the thread as a number and turned back into a handle there.
    let first_addr = first as usize;
    std::thread::Builder::new().name("control".into()).spawn(move || pipe.run(first_addr as HANDLE, key, handler))?;
    Ok(())
}
