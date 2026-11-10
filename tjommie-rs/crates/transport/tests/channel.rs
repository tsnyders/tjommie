//! Port of `packages/client/src/encrypted-channel.test.ts`.

use transport::channel::{Cipher, EncryptedMessages, FrameDecoder, length_prefixed};
use transport::{Failed, Session, generate_keypair};

/// A bounded test double isolates record framing. Actual authentication, tamper and replay tests use Snow.
#[derive(Default)]
struct Plain {
    closed: bool,
}

impl Cipher for Plain {
    fn encrypt(&mut self, plain: &[u8]) -> Result<Vec<u8>, Failed> {
        if self.closed {
            return Err(Failed);
        }
        let mut frame = vec![0; 16];
        frame.extend_from_slice(plain);
        Ok(frame)
    }
    fn decrypt(&mut self, cipher: &[u8]) -> Result<Vec<u8>, Failed> {
        if self.closed {
            return Err(Failed);
        }
        Ok(cipher[16..].to_vec())
    }
    fn close(&mut self) {
        self.closed = true;
    }
}

#[test]
fn round_trips_fragmented_unicode_messages_and_arbitrary_tcp_packet_boundaries() {
    let mut sender = EncryptedMessages::new(Plain::default());
    let mut receiver = EncryptedMessages::new(Plain::default());
    let text = "🙂".repeat(50_000);
    let frames = sender.encode(&text).unwrap();
    assert!(frames.len() > 1);
    let mut decoder = FrameDecoder::default();
    let mut messages = Vec::new();
    for frame in &frames {
        let packet = length_prefixed(frame).unwrap();
        for chunk in packet.chunks(37) {
            for decoded in decoder.feed(chunk).unwrap() {
                if let Some(message) = receiver.receive(&decoded).unwrap() {
                    messages.push(message);
                }
            }
        }
    }
    assert_eq!(messages, [text]);
}

#[test]
fn rejects_oversized_lengths_before_allocation_and_overlapping_message_starts() {
    assert!(FrameDecoder::default().feed(&[127, 255, 255, 255]).is_err());
    let mut sender = EncryptedMessages::new(Plain::default());
    let mut receiver = EncryptedMessages::new(Plain::default());
    let frames = sender.encode(&"x".repeat(70_000)).unwrap();
    assert_eq!(receiver.receive(&frames[0]).unwrap(), None);
    assert!(receiver.receive(&frames[0]).is_err());
    assert!(receiver.receive(&frames[1]).is_err());
}

#[test]
fn does_not_accept_continuation_records_out_of_order() {
    let frames = EncryptedMessages::new(Plain::default())
        .encode(&"x".repeat(70_000))
        .unwrap();
    assert!(
        EncryptedMessages::new(Plain::default())
            .receive(&frames[1])
            .is_err()
    );
}

/// LAN framing is a 4-byte big-endian length (`lengthPrefixed`), and a zero length is refused.
#[test]
fn prefixes_each_frame_with_a_four_byte_big_endian_length() {
    assert_eq!(length_prefixed(&[1, 2]).unwrap(), [0, 0, 0, 2, 1, 2]);
    assert!(length_prefixed(&[]).is_err());
    assert!(length_prefixed(&vec![0; 65_536]).is_err());
    assert!(FrameDecoder::default().feed(&[0, 0, 0, 0]).is_err());
    let mut decoder = FrameDecoder::default();
    assert_eq!(decoder.feed(&[0, 0]).unwrap(), Vec::<Vec<u8>>::new());
    assert_eq!(
        decoder.feed(&[0, 1, 9, 0, 0, 0, 1, 8]).unwrap(),
        [vec![9], vec![8]]
    );
}

#[test]
fn refuses_empty_messages_and_sends_nothing_after_a_failure() {
    let mut sender = EncryptedMessages::new(Plain::default());
    assert_eq!(
        sender.encode("").unwrap_err().to_string(),
        "The encrypted connection could not send this message."
    );
    assert!(sender.encode("x").is_err());
    let mut receiver = EncryptedMessages::new(Plain::default());
    assert_eq!(
        receiver.receive(&[0; 4]).unwrap_err().to_string(),
        "The encrypted connection received an invalid message."
    );
}

#[test]
fn carries_messages_over_a_real_noise_session() {
    let (device, host) = (generate_keypair().unwrap(), generate_keypair().unwrap());
    let mut client =
        Session::initiator(&device.private_key[..], &host.public_key, None, b"").unwrap();
    let mut server = Session::responder(&host.private_key[..], None, b"").unwrap();
    server
        .read_handshake(&client.write_handshake(b"").unwrap())
        .unwrap();
    client
        .read_handshake(&server.write_handshake(b"").unwrap())
        .unwrap();
    let mut sender = EncryptedMessages::new(client);
    let mut receiver = EncryptedMessages::new(server);
    let text = "é".repeat(40_000);
    let mut received = None;
    for frame in sender.encode(&text).unwrap() {
        received = receiver.receive(&frame).unwrap();
    }
    assert_eq!(received, Some(text));
    assert_eq!(
        receiver.cipher().remote_public_key(),
        Some(device.public_key)
    );
}
