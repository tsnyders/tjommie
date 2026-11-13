//! Port of `packages/transport/src/noise.test.ts`: the same cases against the native Snow wrapper.

use transport::{Failed, Keypair, MAX_NOISE_FRAME, MAX_NOISE_PLAINTEXT, Session, generate_keypair};

struct Peers {
    client: Session,
    server: Session,
    device: Keypair,
    host: Keypair,
}

fn peers(psk: Option<&[u8]>) -> Peers {
    peers_with(psk, b"synthetic-context")
}

fn peers_with(psk: Option<&[u8]>, prologue: &[u8]) -> Peers {
    let device = generate_keypair().unwrap();
    let host = generate_keypair().unwrap();
    let client =
        Session::initiator(&device.private_key[..], &host.public_key, psk, prologue).unwrap();
    let server = Session::responder(&host.private_key[..], psk, prologue).unwrap();
    Peers {
        client,
        server,
        device,
        host,
    }
}

fn handshake(pair: &mut Peers) {
    let first = pair.client.write_handshake(b"hello").unwrap();
    assert_eq!(pair.server.read_handshake(&first).unwrap(), b"hello");
    let second = pair.server.write_handshake(b"reply").unwrap();
    assert_eq!(pair.client.read_handshake(&second).unwrap(), b"reply");
}

fn random32() -> [u8; 32] {
    generate_keypair().unwrap().public_key
}

#[test]
fn performs_the_actual_ik_handshake_and_authenticated_bidirectional_encryption() {
    for pairing in [false, true] {
        let psk = random32();
        let mut pair = peers(pairing.then_some(&psk[..]));
        assert!(!pair.client.handshake_finished());
        assert_eq!(pair.server.remote_public_key(), None);
        handshake(&mut pair);
        assert!(pair.client.handshake_finished());
        assert!(pair.server.handshake_finished());
        assert_eq!(pair.client.remote_public_key(), Some(pair.host.public_key));
        assert_eq!(
            pair.server.remote_public_key(),
            Some(pair.device.public_key)
        );
        let encrypted = pair.client.encrypt(b"Synthetic RPC data").unwrap();
        assert_ne!(encrypted, b"Synthetic RPC data");
        assert_eq!(
            pair.server.decrypt(&encrypted).unwrap(),
            b"Synthetic RPC data"
        );
        let reply = pair.server.encrypt(b"Synthetic result").unwrap();
        assert_eq!(pair.client.decrypt(&reply).unwrap(), b"Synthetic result");
    }
}

#[test]
fn generates_independent_32_byte_key_pairs() {
    let (a, b) = (generate_keypair().unwrap(), generate_keypair().unwrap());
    assert_ne!(a.private_key[..], a.public_key[..]);
    assert_ne!(a.private_key[..], b.private_key[..]);
    assert_ne!(a.public_key, b.public_key);
}

#[test]
fn ikpsk2_responder_handshake_completion_alone_does_not_prove_the_clients_psk() {
    let (device, host) = (generate_keypair().unwrap(), generate_keypair().unwrap());
    let mut client = Session::initiator(
        &device.private_key[..],
        &host.public_key,
        Some(&[1; 32]),
        b"",
    )
    .unwrap();
    let mut server = Session::responder(&host.private_key[..], Some(&[2; 32]), b"").unwrap();
    // This succeeds without knowledge of the correct PSK. Application pairing MUST NOT finish here.
    server
        .read_handshake(&client.write_handshake(b"").unwrap())
        .unwrap();
    let response = server.write_handshake(b"").unwrap();
    assert!(server.handshake_finished());
    assert_eq!(client.read_handshake(&response), Err(Failed));
    assert_eq!(client.encrypt(b"confirm"), Err(Failed));
}

#[test]
fn only_an_encrypted_confirmation_authenticates_the_new_pairing() {
    let mut pair = peers(Some(&[3; 32]));
    handshake(&mut pair);
    let confirm = pair
        .client
        .encrypt(br#"{"type":"confirm","protocol":1}"#)
        .unwrap();
    assert_eq!(
        pair.server.decrypt(&confirm).unwrap(),
        br#"{"type":"confirm","protocol":1}"#
    );
}

#[test]
fn rejects_a_wrong_pinned_host_key_and_closes_the_failed_session() {
    let mut pair = peers(None);
    let wrong = generate_keypair().unwrap();
    let mut attacker = Session::initiator(
        &pair.device.private_key[..],
        &wrong.public_key,
        None,
        b"synthetic-context",
    )
    .unwrap();
    let frame = attacker.write_handshake(b"").unwrap();
    assert_eq!(pair.server.read_handshake(&frame), Err(Failed));
    assert_eq!(pair.server.remote_public_key(), None);
    assert_eq!(pair.server.write_handshake(b""), Err(Failed));
}

#[test]
fn binds_routing_context_and_refuses_downgrade_between_pairing_and_reconnect() {
    let mut pair = peers(Some(&[4; 32]));
    let frame = pair.client.write_handshake(b"").unwrap();
    let mut changed =
        Session::responder(&pair.host.private_key[..], Some(&[4; 32]), b"other-host").unwrap();
    assert_eq!(changed.read_handshake(&frame), Err(Failed));
    let mut downgraded =
        Session::responder(&pair.host.private_key[..], None, b"synthetic-context").unwrap();
    assert_eq!(downgraded.read_handshake(&frame), Err(Failed));
}

#[test]
fn fails_closed_on_tamper_replay_and_reorder_without_allowing_nonce_retries() {
    for attack in ["tamper", "replay", "reorder"] {
        let mut pair = peers(None);
        handshake(&mut pair);
        let mut one = pair.client.encrypt(b"one").unwrap();
        let two = pair.client.encrypt(b"two").unwrap();
        if attack == "tamper" {
            one[0] ^= 1;
        }
        if attack == "replay" {
            pair.server.decrypt(&one).unwrap();
        }
        let first = if attack == "reorder" { &two } else { &one };
        assert_eq!(pair.server.decrypt(first), Err(Failed), "{attack}");
        assert_eq!(pair.server.decrypt(&two), Err(Failed), "{attack}");
    }
}

#[test]
fn fresh_reconnect_uses_fresh_ephemeral_keys_and_rejects_prior_session_ciphertext() {
    let mut pair = peers(None);
    handshake(&mut pair);
    let previous = pair.client.encrypt(b"private message").unwrap();
    let mut client = Session::initiator(
        &pair.device.private_key[..],
        &pair.host.public_key,
        None,
        b"",
    )
    .unwrap();
    let mut server = Session::responder(&pair.host.private_key[..], None, b"").unwrap();
    server
        .read_handshake(&client.write_handshake(b"").unwrap())
        .unwrap();
    client
        .read_handshake(&server.write_handshake(b"").unwrap())
        .unwrap();
    assert_ne!(client.encrypt(b"private message").unwrap(), previous);
    assert_eq!(server.decrypt(&previous), Err(Failed));
}

#[test]
fn supports_the_maximum_noise_record_and_refuses_one_excess_plaintext_byte() {
    let mut pair = peers(None);
    handshake(&mut pair);
    let plain = vec![5; MAX_NOISE_PLAINTEXT];
    let encrypted = pair.client.encrypt(&plain).unwrap();
    assert_eq!(encrypted.len(), MAX_NOISE_FRAME);
    assert_eq!(pair.server.decrypt(&encrypted).unwrap(), plain);
    assert_eq!(
        pair.client.encrypt(&vec![0; MAX_NOISE_PLAINTEXT + 1]),
        Err(Failed)
    );
    // The failure closed the session, as the TypeScript wrapper does.
    assert_eq!(pair.client.encrypt(b""), Err(Failed));
}

#[test]
fn rejects_malformed_ciphertext_lengths() {
    for length in [0, 15, MAX_NOISE_FRAME + 1] {
        let mut pair = peers(None);
        handshake(&mut pair);
        assert_eq!(
            pair.server.decrypt(&vec![0; length]),
            Err(Failed),
            "{length}"
        );
    }
}

#[test]
fn rejects_oversized_handshakes_invalid_phase_calls_and_closed_sessions() {
    let mut pair = peers(None);
    assert_eq!(
        pair.server.read_handshake(&vec![0; MAX_NOISE_FRAME + 1]),
        Err(Failed)
    );
    assert_eq!(pair.client.encrypt(b""), Err(Failed));
    let mut ready = peers(None);
    handshake(&mut ready);
    assert_eq!(ready.client.write_handshake(b""), Err(Failed));
    ready.server.close();
    ready.server.close();
    assert!(!ready.server.handshake_finished());
    assert_eq!(ready.server.remote_public_key(), None);
    assert_eq!(ready.server.decrypt(&[0; 16]), Err(Failed));
}

#[test]
fn rejects_invalid_key_and_psk_lengths() {
    let key = generate_keypair().unwrap();
    for length in [0, 31, 33, 64] {
        let bad = vec![0; length];
        assert!(
            Session::initiator(&bad, &key.public_key, None, b"").is_err(),
            "{length}"
        );
        assert!(
            Session::initiator(&key.private_key[..], &bad, None, b"").is_err(),
            "{length}"
        );
        assert!(
            Session::responder(&key.private_key[..], Some(&bad), b"").is_err(),
            "{length}"
        );
    }
}

#[test]
fn errors_carry_the_fixed_message() {
    assert_eq!(Failed.to_string(), "Secure connection failed.");
}

/// `index.ts` refuses a caller prologue over 4096 bytes, and the native module refuses a domain-separated prologue over
/// 4096 bytes, so together a caller gets 4096 minus the 18-byte `Tjommie/remote/v1\0` prefix.
#[test]
fn bounds_the_domain_separated_prologue_like_the_typescript_wrapper() {
    let key = generate_keypair().unwrap();
    assert!(Session::responder(&key.private_key[..], None, &[0; 4078]).is_ok());
    assert!(Session::responder(&key.private_key[..], None, &[0; 4079]).is_err());
}
