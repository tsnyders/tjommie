//! Port of `packages/transport/native/src/lib.rs` without wasm-bindgen, plus the domain separation that
//! `packages/transport/src/index.ts` adds around it. Thin bindings only: Snow owns all handshake, key agreement, AEAD and
//! nonce state. Every failure is the same opaque [`Failed`], and a failed session stays closed.

use std::fmt;

use snow::{Builder, HandshakeState, TransportState};
use zeroize::Zeroizing;

pub mod channel;

const IK: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
const IK_PSK: &str = "Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
pub const MAX_NOISE_FRAME: usize = 65_535;
pub const MAX_NOISE_PLAINTEXT: usize = MAX_NOISE_FRAME - 16;
pub const MAX_NOISE_RECORDS: u64 = 1 << 31;
/// `index.ts`: the application context is always domain-separated, including callers that supply routing bytes.
const DOMAIN: &[u8] = b"Tjommie/remote/v1\0";
const MAX_PROLOGUE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed;

impl fmt::Display for Failed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secure connection failed.")
    }
}

impl std::error::Error for Failed {}

/// An X25519 key pair. The private half is wiped when dropped.
pub struct Keypair {
    pub private_key: Zeroizing<[u8; 32]>,
    pub public_key: [u8; 32],
}

pub fn generate_keypair() -> Result<Keypair, Failed> {
    let keys = Builder::new(IK.parse().map_err(|_| Failed)?)
        .generate_keypair()
        .map_err(|_| Failed)?;
    let private = Zeroizing::new(keys.private);
    Ok(Keypair {
        private_key: Zeroizing::new(private.as_slice().try_into().map_err(|_| Failed)?),
        public_key: keys.public.as_slice().try_into().map_err(|_| Failed)?,
    })
}

enum State {
    Handshake(Box<HandshakeState>),
    Transport(Box<TransportState>),
}

/// One side of a Noise IK (or IKpsk2, when a pairing secret is given) session.
pub struct Session {
    state: Option<State>,
}

impl Session {
    pub fn initiator(
        private_key: &[u8],
        remote_public_key: &[u8],
        psk: Option<&[u8]>,
        prologue: &[u8],
    ) -> Result<Session, Failed> {
        create(true, private_key, Some(remote_public_key), psk, prologue)
    }

    pub fn responder(
        private_key: &[u8],
        psk: Option<&[u8]>,
        prologue: &[u8],
    ) -> Result<Session, Failed> {
        create(false, private_key, None, psk, prologue)
    }

    pub fn handshake_finished(&self) -> bool {
        matches!(self.state, Some(State::Transport(_)))
    }

    pub fn remote_public_key(&self) -> Option<[u8; 32]> {
        let key = match self.state.as_ref()? {
            State::Handshake(state) => state.get_remote_static(),
            State::Transport(state) => state.get_remote_static(),
        };
        key?.try_into().ok()
    }

    pub fn write_handshake(&mut self, payload: &[u8]) -> Result<Vec<u8>, Failed> {
        self.handshake_message(payload, true)
    }

    pub fn read_handshake(&mut self, frame: &[u8]) -> Result<Vec<u8>, Failed> {
        self.handshake_message(frame, false)
    }

    pub fn encrypt(&mut self, plain: &[u8]) -> Result<Vec<u8>, Failed> {
        self.transport_message(plain, true)
    }

    pub fn decrypt(&mut self, cipher: &[u8]) -> Result<Vec<u8>, Failed> {
        self.transport_message(cipher, false)
    }

    pub fn close(&mut self) {
        self.state = None;
    }

    // Both helpers take the state out first, so any failure leaves the session closed.
    fn handshake_message(&mut self, input: &[u8], writing: bool) -> Result<Vec<u8>, Failed> {
        let Some(State::Handshake(mut state)) = self.state.take() else {
            return Err(Failed);
        };
        if input.len() > MAX_NOISE_FRAME {
            return Err(Failed);
        }
        let mut output = vec![0; MAX_NOISE_FRAME];
        let length = if writing {
            state.write_message(input, &mut output)
        } else {
            state.read_message(input, &mut output)
        }
        .map_err(|_| Failed)?;
        output.truncate(length);
        self.state = Some(if state.is_handshake_finished() {
            State::Transport(Box::new(state.into_transport_mode().map_err(|_| Failed)?))
        } else {
            State::Handshake(state)
        });
        Ok(output)
    }

    fn transport_message(&mut self, input: &[u8], writing: bool) -> Result<Vec<u8>, Failed> {
        let Some(State::Transport(mut state)) = self.state.take() else {
            return Err(Failed);
        };
        let refused = if writing {
            input.len() > MAX_NOISE_PLAINTEXT || state.sending_nonce() >= MAX_NOISE_RECORDS
        } else {
            input.len() < 16
                || input.len() > MAX_NOISE_FRAME
                || state.receiving_nonce() >= MAX_NOISE_RECORDS
        };
        if refused {
            return Err(Failed);
        }
        let mut output = vec![0; MAX_NOISE_FRAME];
        let length = if writing {
            state.write_message(input, &mut output)
        } else {
            state.read_message(input, &mut output)
        }
        .map_err(|_| Failed)?;
        output.truncate(length);
        self.state = Some(State::Transport(state));
        Ok(output)
    }
}

fn create(
    initiating: bool,
    private_key: &[u8],
    remote_key: Option<&[u8]>,
    psk: Option<&[u8]>,
    prologue: &[u8],
) -> Result<Session, Failed> {
    let mut context = DOMAIN.to_vec();
    context.extend_from_slice(prologue);
    if private_key.len() != 32
        || remote_key.is_some_and(|key| key.len() != 32)
        || context.len() > MAX_PROLOGUE
    {
        return Err(Failed);
    }
    let secret: Option<&[u8; 32]> = match psk {
        Some(key) => Some(key.try_into().map_err(|_| Failed)?),
        None => None,
    };
    let name = if secret.is_some() { IK_PSK } else { IK };
    let mut builder = Builder::new(name.parse().map_err(|_| Failed)?)
        .local_private_key(private_key)
        .map_err(|_| Failed)?
        .prologue(&context)
        .map_err(|_| Failed)?;
    if let Some(key) = remote_key {
        builder = builder.remote_public_key(key).map_err(|_| Failed)?;
    }
    if let Some(key) = secret {
        builder = builder.psk(2, key).map_err(|_| Failed)?;
    }
    let handshake = if initiating {
        builder.build_initiator()
    } else {
        builder.build_responder()
    }
    .map_err(|_| Failed)?;
    Ok(Session {
        state: Some(State::Handshake(Box::new(handshake))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    // Published Cacophony vectors, distributed in Snow v0.10.0 tests/vectors/cacophony.txt.
    // Fixed ephemerals are test-only and never reachable through the wrapper.
    #[test]
    fn published_ik_and_ikpsk2_vectors_match_handshake_and_transport() {
        let init_static = hex("e61ef9919cde45dd5f82166404bd08e38bceb5dfdfded0a34c8df7ed542214d1");
        let init_ephemeral =
            hex("893e28b9dc6ca8d611ab664754b8ceb7bac5117349a4439a6b0569da977c464a");
        let remote_static = hex("31e0303fd6418d2f8c0e78b91f22e8caed0fbe48656dcf4767e4834f701b8f62");
        let resp_static = hex("4a3acbfdb163dec651dfa3194dece676d437029c62a408b4c5ea9114246e4893");
        let resp_ephemeral =
            hex("bbdb4cdbd309f1a1f2e1456967fe288cadd6f712d65dc7b7793d5e63da6b375b");
        let psk: [u8; 32] = hex("54686973206973206d7920417573747269616e20706572737065637469766521")
            .try_into()
            .unwrap();
        for (name, first, second, hash, third) in [
            (
                IK,
                "ca35def5ae56cec33dc2036731ab14896bc4c75dbb07a61f879f8e3afa4c79440b03ddc7aac5123d06a1b23b71670e32e76c28239a7ca4ac8f784de7e44c1adbfc6e83fef7352a58d9d56157400c0a737b1d171ce368229c7b752ac25b8faf4eca690f6d896f543be02c996ab2b86b76",
                "95ebc60d2b1fa672c1f46a8aa265ef51bfe38e7ccb39ec5be34069f144808843d9b5a8927f0ac9655ef76833bc7e5561f42e691ac8404efd6fbd6308b6a27c",
                "48f3cb8bc9319da4ba1e9933991b1c4ed4034f1f126a76d3a1fbcfd7f94248d4",
                "2c256ed08fcd08c2980f954ee4beaccb61c9581340f5dd2fd1cf3b",
            ),
            (
                IK_PSK,
                "ca35def5ae56cec33dc2036731ab14896bc4c75dbb07a61f879f8e3afa4c7944001e21de9f98ddd8e2ad57527207feb56253c9c94a9e496782ecfcb2a75fbcaf1b52948cc48daefe660c62119ab5000980c84831215f2441eba616548e832985464cf17e51ee93109008399a21f7e13f",
                "95ebc60d2b1fa672c1f46a8aa265ef51bfe38e7ccb39ec5be34069f144808843cb765f2caef0751b8f007572dab0322217755c0632f365717edbf34d33e87a",
                "f5191b875290abcd41347ac3622d9679688a7e980229cb937ef748336cfde0e5",
                "8153ca9833bc3c1b91a7e66e5f4d4f5b59bf9e64c2f20d15f0bba7",
            ),
        ] {
            let mut initiator = Builder::new(name.parse().unwrap())
                .local_private_key(&init_static)
                .unwrap()
                .remote_public_key(&remote_static)
                .unwrap()
                .prologue(b"John Galt")
                .unwrap()
                .fixed_ephemeral_key_for_testing_only(&init_ephemeral);
            let mut responder = Builder::new(name.parse().unwrap())
                .local_private_key(&resp_static)
                .unwrap()
                .prologue(b"John Galt")
                .unwrap()
                .fixed_ephemeral_key_for_testing_only(&resp_ephemeral);
            if name == IK_PSK {
                initiator = initiator.psk(2, &psk).unwrap();
                responder = responder.psk(2, &psk).unwrap();
            }
            let mut init = initiator.build_initiator().unwrap();
            let mut resp = responder.build_responder().unwrap();
            let mut frame = [0; MAX_NOISE_FRAME];
            let mut plain = [0; MAX_NOISE_FRAME];
            let length = init.write_message(b"Ludwig von Mises", &mut frame).unwrap();
            assert_eq!(&frame[..length], hex(first));
            let got = resp.read_message(&frame[..length], &mut plain).unwrap();
            assert_eq!(&plain[..got], b"Ludwig von Mises");
            let length = resp.write_message(b"Murray Rothbard", &mut frame).unwrap();
            assert_eq!(&frame[..length], hex(second));
            init.read_message(&frame[..length], &mut plain).unwrap();
            assert_eq!(init.get_handshake_hash(), hex(hash));
            let mut init = init.into_transport_mode().unwrap();
            let mut resp = resp.into_transport_mode().unwrap();
            let length = init.write_message(b"F. A. Hayek", &mut frame).unwrap();
            assert_eq!(&frame[..length], hex(third));
            let got = resp.read_message(&frame[..length], &mut plain).unwrap();
            assert_eq!(&plain[..got], b"F. A. Hayek");
        }
    }

    #[test]
    fn exact_protocols_are_supported_by_snow() {
        for name in [IK, IK_PSK] {
            let mut builder = Builder::new(name.parse().unwrap());
            let keys = builder.generate_keypair().unwrap();
            assert_eq!(keys.private.len(), 32);
            assert_eq!(keys.public.len(), 32);
            builder = builder.local_private_key(&keys.private).unwrap();
            let psk = [7; 32];
            if name == IK_PSK {
                builder = builder.psk(2, &psk).unwrap();
            }
            assert!(builder.build_responder().is_ok());
        }
    }
}
