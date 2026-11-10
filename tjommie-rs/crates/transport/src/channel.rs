//! Port of `packages/client/src/encrypted-channel.ts`: message framing over Noise records, and the 4-byte length prefix
//! that carries Noise records over a LAN TCP stream. Framing only: Snow owns authentication, encryption and ordered
//! nonces. A message cannot interleave with another; all of its records come from one `encode` call.

use std::fmt;

use zeroize::Zeroizing;

use crate::{Failed, MAX_NOISE_FRAME, Session};

pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// The largest Noise plaintext (65519) minus the 5-byte record header.
const CHUNK_BYTES: usize = 65_519 - 5;

pub trait Cipher {
    fn encrypt(&mut self, plain: &[u8]) -> Result<Vec<u8>, Failed>;
    fn decrypt(&mut self, cipher: &[u8]) -> Result<Vec<u8>, Failed>;
    fn close(&mut self);
}

impl Cipher for Session {
    fn encrypt(&mut self, plain: &[u8]) -> Result<Vec<u8>, Failed> {
        Session::encrypt(self, plain)
    }
    fn decrypt(&mut self, cipher: &[u8]) -> Result<Vec<u8>, Failed> {
        Session::decrypt(self, cipher)
    }
    fn close(&mut self) {
        Session::close(self);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    Send,
    Receive,
}

impl fmt::Display for ChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            ChannelError::Send => "The encrypted connection could not send this message.",
            ChannelError::Receive => "The encrypted connection received an invalid message.",
        })
    }
}

impl std::error::Error for ChannelError {}

/// Text messages split into records of at most one Noise frame. The first record carries flag 1 and the total length,
/// each later one flag 0 and its offset. Any failure closes the channel for good.
pub struct EncryptedMessages<C: Cipher> {
    cipher: C,
    buffer: Option<Zeroizing<Vec<u8>>>,
    at: usize,
    closed: bool,
}

impl<C: Cipher> EncryptedMessages<C> {
    pub fn new(cipher: C) -> Self {
        Self {
            cipher,
            buffer: None,
            at: 0,
            closed: false,
        }
    }

    pub fn cipher(&self) -> &C {
        &self.cipher
    }

    pub fn encode(&mut self, text: &str) -> Result<Vec<Vec<u8>>, ChannelError> {
        let result = (|| {
            let bytes = text.as_bytes();
            if self.closed || bytes.is_empty() || bytes.len() > MAX_MESSAGE_BYTES {
                return Err(Failed);
            }
            let mut frames = Vec::with_capacity(bytes.len().div_ceil(CHUNK_BYTES));
            for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
                let at = index * CHUNK_BYTES;
                let mut payload = Zeroizing::new(Vec::with_capacity(chunk.len() + 5));
                payload.push(u8::from(at == 0));
                let position = if at == 0 { bytes.len() } else { at };
                payload.extend_from_slice(&(position as u32).to_be_bytes());
                payload.extend_from_slice(chunk);
                frames.push(self.cipher.encrypt(&payload)?);
            }
            Ok(frames)
        })();
        result.map_err(|Failed| {
            self.close();
            ChannelError::Send
        })
    }

    /// The whole message once its last record arrives, `None` before that.
    pub fn receive(&mut self, frame: &[u8]) -> Result<Option<String>, ChannelError> {
        let result = (|| {
            if self.closed || frame.len() < 16 || frame.len() > MAX_NOISE_FRAME {
                return Err(Failed);
            }
            let payload = Zeroizing::new(self.cipher.decrypt(frame)?);
            if payload.len() <= 5 {
                return Err(Failed);
            }
            let position =
                u32::from_be_bytes(payload[1..5].try_into().map_err(|_| Failed)?) as usize;
            match (payload[0], &self.buffer) {
                (1, None) if position != 0 && position <= MAX_MESSAGE_BYTES => {
                    self.buffer = Some(Zeroizing::new(vec![0; position]));
                    self.at = 0;
                }
                (0, Some(_)) if position == self.at => {}
                _ => return Err(Failed),
            }
            let buffer = self.buffer.as_mut().ok_or(Failed)?;
            let body = &payload[5..];
            if self.at + body.len() > buffer.len() {
                return Err(Failed);
            }
            buffer[self.at..self.at + body.len()].copy_from_slice(body);
            self.at += body.len();
            if self.at != buffer.len() {
                return Ok(None);
            }
            let text = std::str::from_utf8(buffer).map_err(|_| Failed)?.to_owned();
            self.buffer = None;
            self.at = 0;
            Ok(Some(text))
        })();
        result.map_err(|Failed| {
            self.close();
            ChannelError::Receive
        })
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.buffer = None;
        self.cipher.close();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidFrame;

impl fmt::Display for InvalidFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Invalid frame size.")
    }
}

impl std::error::Error for InvalidFrame {}

/// `lengthPrefixed`: a 4-byte big-endian length, then the frame.
pub fn length_prefixed(frame: &[u8]) -> Result<Vec<u8>, InvalidFrame> {
    if frame.is_empty() || frame.len() > MAX_NOISE_FRAME {
        return Err(InvalidFrame);
    }
    let mut bytes = Vec::with_capacity(frame.len() + 4);
    bytes.extend_from_slice(&(frame.len() as u32).to_be_bytes());
    bytes.extend_from_slice(frame);
    Ok(bytes)
}

/// `FrameDecoder`: turns arbitrary TCP chunks back into frames. A bad length fails before anything is allocated.
#[derive(Default)]
pub struct FrameDecoder {
    header: [u8; 4],
    header_at: usize,
    body: Option<Vec<u8>>,
}

impl FrameDecoder {
    pub fn feed(&mut self, mut chunk: &[u8]) -> Result<Vec<Vec<u8>>, InvalidFrame> {
        let mut frames = Vec::new();
        while !chunk.is_empty() {
            let body = match &mut self.body {
                Some(body) => body,
                None => {
                    let take = (4 - self.header_at).min(chunk.len());
                    self.header[self.header_at..self.header_at + take]
                        .copy_from_slice(&chunk[..take]);
                    chunk = &chunk[take..];
                    self.header_at += take;
                    if self.header_at < 4 {
                        continue;
                    }
                    self.header_at = 0;
                    let length = u32::from_be_bytes(self.header) as usize;
                    if length == 0 || length > MAX_NOISE_FRAME {
                        return Err(InvalidFrame);
                    }
                    self.body.insert(Vec::with_capacity(length))
                }
            };
            let length = u32::from_be_bytes(self.header) as usize;
            let take = (length - body.len()).min(chunk.len());
            body.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
            if body.len() == length {
                frames.extend(self.body.take());
            }
        }
        Ok(frames)
    }
}
