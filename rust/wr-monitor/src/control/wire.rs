//! The wire format of Python's `multiprocessing.connection`, which the White Rabbit window's
//! `led_mon/control.py` speaks: messages, and the challenge/response handshake that proves both ends
//! know the key in control.key. A port of `Connection._send_bytes` / `_recv_bytes`, `deliver_challenge`
//! and `answer_challenge` in CPython's `Lib/multiprocessing/connection.py` (3.12).
//!
//! On a socket each message is a 4-byte big-endian length and the bytes (a length of -1 means an
//! 8-byte length follows, for messages over 2 GB). A Windows named pipe in message mode keeps the
//! messages apart by itself, so there is no length there (see `pipe.rs`).

use std::io;
#[cfg(any(unix, test))]
use std::io::{Read, Write};

use hmac::{Hmac, Mac};

/// Sends and receives whole messages.
pub trait MessageStream {
    fn send_message(&mut self, data: &[u8]) -> io::Result<()>;
    /// The next message; an error when it is longer than `max` bytes.
    fn recv_message(&mut self, max: usize) -> io::Result<Vec<u8>>;
}

/// A byte stream (a Unix socket, or anything in the tests) with Python's length before each message.
#[cfg(any(unix, test))]
pub struct Framed<S>(pub S);

#[cfg(any(unix, test))]
impl<S: Read + Write> MessageStream for Framed<S> {
    fn send_message(&mut self, data: &[u8]) -> io::Result<()> {
        let mut header = Vec::with_capacity(12);
        if data.len() > 0x7fff_ffff {
            header.extend_from_slice(&(-1i32).to_be_bytes());
            header.extend_from_slice(&(data.len() as u64).to_be_bytes());
        } else {
            header.extend_from_slice(&(data.len() as i32).to_be_bytes());
        }
        self.0.write_all(&header)?;
        self.0.write_all(data)?;
        self.0.flush()
    }

    fn recv_message(&mut self, max: usize) -> io::Result<Vec<u8>> {
        let mut four = [0u8; 4];
        self.0.read_exact(&mut four)?;
        let size = match i32::from_be_bytes(four) {
            -1 => {
                let mut eight = [0u8; 8];
                self.0.read_exact(&mut eight)?;
                u64::from_be_bytes(eight)
            }
            n if n < 0 => return Err(bad_data("bad message length")),
            n => n as u64,
        };
        if size > max as u64 {
            return Err(bad_data("bad message length"));
        }
        let mut data = vec![0u8; size as usize];
        self.0.read_exact(&mut data)?;
        Ok(data)
    }
}

#[cfg(any(unix, test))]
fn bad_data(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text.to_string())
}

fn auth_error(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, text.to_string())
}

const CHALLENGE: &[u8] = b"#CHALLENGE#";
const WELCOME: &[u8] = b"#WELCOME#";
const FAILURE: &[u8] = b"#FAILURE#";
/// Random bytes in a challenge we send.
const MESSAGE_LENGTH: usize = 40;
/// Python before 3.12 sends 20-byte challenges and answers with a bare 16-byte HMAC-MD5.
const MD5ONLY_MESSAGE_LENGTH: usize = 20;
const MD5_DIGEST_LENGTH: usize = 16;
/// The longest digest name Python allows ("sha3_256").
const MAX_DIGEST_NAME: usize = 8;
/// Handshake messages are short; a longer one is refused, as in Python.
const MAX_HANDSHAKE: usize = 256;

/// The HMAC digests we can make. Python also allows the sha3 ones, which it never picks by itself.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Digest {
    Md5,
    Sha256,
    Sha384,
}

impl Digest {
    fn from_name(name: &[u8]) -> Option<Digest> {
        match name {
            b"md5" => Some(Digest::Md5),
            b"sha256" => Some(Digest::Sha256),
            b"sha384" => Some(Digest::Sha384),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Digest::Md5 => "md5",
            Digest::Sha256 => "sha256",
            Digest::Sha384 => "sha384",
        }
    }

    fn mac(self, key: &[u8], message: &[u8]) -> Vec<u8> {
        fn run<M: Mac + hmac::digest::KeyInit>(key: &[u8], message: &[u8]) -> Vec<u8> {
            let mut mac = <M as hmac::digest::KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
            mac.update(message);
            mac.finalize().into_bytes().to_vec()
        }
        match self {
            Digest::Md5 => run::<Hmac<md5::Md5>>(key, message),
            Digest::Sha256 => run::<Hmac<sha2::Sha256>>(key, message),
            Digest::Sha384 => run::<Hmac<sha2::Sha384>>(key, message),
        }
    }
}

/// Python's `_get_digest_name_and_payload`: None for the old bare form (MD5), else the digest named
/// in the `{name}` prefix, and the bytes after it.
fn digest_and_payload(message: &[u8]) -> io::Result<(Option<Digest>, &[u8])> {
    if message.len() == MD5ONLY_MESSAGE_LENGTH || message.len() == MD5_DIGEST_LENGTH {
        return Ok((None, message));
    }
    if message.first() == Some(&b'{') {
        let end = message.len().min(MAX_DIGEST_NAME + 2);
        if let Some(curly) = message[1..end].iter().position(|&b| b == b'}').map(|i| i + 1) {
            if let Some(digest) = Digest::from_name(&message[1..curly]) {
                return Ok((Some(digest), &message[curly + 1..]));
            }
        }
    }
    Err(auth_error("unsupported message length, missing digest prefix, or unsupported digest"))
}

/// Python's `_create_response`: the HMAC of the whole challenge (its `{name}` prefix too).
fn create_response(key: &[u8], challenge: &[u8]) -> io::Result<Vec<u8>> {
    match digest_and_payload(challenge)?.0 {
        None => Ok(Digest::Md5.mac(key, challenge)),
        Some(digest) => {
            let mut response = format!("{{{}}}", digest.name()).into_bytes();
            response.extend(digest.mac(key, challenge));
            Ok(response)
        }
    }
}

/// Python's `_verify_challenge`: the other end's answer to our challenge `message`.
fn verify_response(key: &[u8], message: &[u8], response: &[u8]) -> io::Result<()> {
    let (digest, mac) = digest_and_payload(response)?;
    let expected = digest.unwrap_or(Digest::Md5).mac(key, message);
    // Compare every byte, so the time taken says nothing about where the first difference is
    let same = expected.len() == mac.len() && expected.iter().zip(mac).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
    if same {
        Ok(())
    } else {
        Err(auth_error("digest received was wrong"))
    }
}

/// Random bytes for challenges and new keys.
pub fn random_bytes(n: usize) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes)
}

/// Python's `deliver_challenge`: we ask, the other end proves it has the key.
pub fn deliver_challenge(conn: &mut dyn MessageStream, key: &[u8]) -> io::Result<()> {
    let mut message = b"{sha256}".to_vec();
    message.extend(random_bytes(MESSAGE_LENGTH)?);
    conn.send_message(&[CHALLENGE, &message].concat())?;
    let response = conn.recv_message(MAX_HANDSHAKE)?;
    match verify_response(key, &message, &response) {
        Ok(()) => conn.send_message(WELCOME),
        Err(e) => {
            let _ = conn.send_message(FAILURE);
            Err(e)
        }
    }
}

/// Python's `answer_challenge`: the other end asks, we prove we have the key.
pub fn answer_challenge(conn: &mut dyn MessageStream, key: &[u8]) -> io::Result<()> {
    let message = conn.recv_message(MAX_HANDSHAKE)?;
    let Some(challenge) = message.strip_prefix(CHALLENGE) else {
        return Err(auth_error("protocol error, expected a challenge"));
    };
    if challenge.len() < MD5ONLY_MESSAGE_LENGTH {
        return Err(auth_error("challenge too short"));
    }
    conn.send_message(&create_response(key, challenge)?)?;
    if conn.recv_message(MAX_HANDSHAKE)? != WELCOME {
        return Err(auth_error("digest sent was rejected"));
    }
    Ok(())
}

/// What a `Listener` with an authkey does on accept: challenge the client, then answer its challenge.
pub fn server_handshake(conn: &mut dyn MessageStream, key: &[u8]) -> io::Result<()> {
    deliver_challenge(conn, key)?;
    answer_challenge(conn, key)
}

/// What `Client(address, authkey=...)` does: the other way round.
#[cfg(test)]
pub fn client_handshake(conn: &mut dyn MessageStream, key: &[u8]) -> io::Result<()> {
    answer_challenge(conn, key)?;
    deliver_challenge(conn, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::mpsc::{channel, Receiver, Sender};

    /// One end of an in-memory pipe that keeps messages apart, like a named pipe.
    struct End(Sender<Vec<u8>>, Receiver<Vec<u8>>);

    fn pair() -> (End, End) {
        let (a_tx, a_rx) = channel();
        let (b_tx, b_rx) = channel();
        (End(a_tx, b_rx), End(b_tx, a_rx))
    }

    impl MessageStream for End {
        fn send_message(&mut self, data: &[u8]) -> io::Result<()> {
            self.0.send(data.to_vec()).map_err(|_| io::ErrorKind::BrokenPipe.into())
        }
        fn recv_message(&mut self, max: usize) -> io::Result<Vec<u8>> {
            let m = self.1.recv().map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?;
            if m.len() > max {
                return Err(bad_data("bad message length"));
            }
            Ok(m)
        }
    }

    fn handshake(server_key: &'static [u8], client_key: &'static [u8]) -> (io::Result<()>, io::Result<()>) {
        let (mut s, mut c) = pair();
        let server = std::thread::spawn(move || server_handshake(&mut s, server_key));
        let client = client_handshake(&mut c, client_key);
        drop(c);
        (server.join().unwrap(), client)
    }

    #[test]
    fn the_same_key_passes_both_ways() {
        let (s, c) = handshake(b"secret", b"secret");
        assert!(s.is_ok() && c.is_ok());
    }

    #[test]
    fn a_wrong_key_is_refused() {
        let (s, c) = handshake(b"secret", b"guess");
        assert_eq!(s.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(c.is_err());
    }

    #[test]
    fn answers_like_python() {
        // hmac.new(b"key", b"{sha256}" + bytes(range(40)), "sha256").digest(), prefixed
        let challenge: Vec<u8> = b"{sha256}".iter().copied().chain(0u8..40).collect();
        let response = create_response(b"key", &challenge).unwrap();
        assert!(response.starts_with(b"{sha256}"));
        assert_eq!(response.len(), 8 + 32);
        verify_response(b"key", &challenge, &response).unwrap();
        // Python 3.11 and older: a bare 20-byte challenge gets a bare HMAC-MD5
        let old: Vec<u8> = (0u8..20).collect();
        let response = create_response(b"key", &old).unwrap();
        assert_eq!(response, Digest::Md5.mac(b"key", &old));
        assert_eq!(response.len(), 16);
        // and its bare answer to our challenge is taken as MD5
        verify_response(b"key", &challenge, &Digest::Md5.mac(b"key", &challenge)).unwrap();
    }

    #[test]
    fn known_hmac_values() {
        // RFC 2104 / RFC 4231 test vectors
        let md5 = Digest::Md5.mac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex(&md5), "750c783e6ab0b503eaa86e310a5db738");
        let sha = Digest::Sha256.mac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex(&sha), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn unknown_digests_are_refused() {
        assert!(digest_and_payload(b"{sha1}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_err());
        assert!(digest_and_payload(b"no prefix and the wrong length").is_err());
    }

    #[test]
    fn framing_matches_python() {
        let mut out = Framed(VecDeque::new());
        out.send_message(b"hello").unwrap();
        assert_eq!(out.0.iter().copied().collect::<Vec<_>>(), b"\0\0\0\x05hello");
        assert_eq!(out.recv_message(16).unwrap(), b"hello");
        // The long form: -1, then an 8-byte length
        let mut long = Framed(VecDeque::from(b"\xff\xff\xff\xff\0\0\0\0\0\0\0\x03abc".to_vec()));
        assert_eq!(long.recv_message(16).unwrap(), b"abc");
        // Too long for what the caller takes
        let mut big = Framed(VecDeque::from(b"\0\0\x01\0".to_vec()));
        assert_eq!(big.recv_message(16).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
