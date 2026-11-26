//! The media viewer on real WebView2 (Phase 3 plan T4.4, `media_viewer::closed_after_close`): a player opens in the
//! profile `media`, reads its file through its source only, in reads of at most 256 KiB, and isn't counted once it
//! closes.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::Ui;
use desktop_host::media::{CHUNK, MediaSource, MediaViewer, Spawn, live_controllers};
use futures::task::LocalSpawnExt as _;

/// A WAV of silence, noting each read.
struct Wav {
    bytes: Vec<u8>,
    reads: Arc<Mutex<Vec<(u64, usize)>>>,
}

impl MediaSource for Wav {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn content_type(&self) -> &str {
        "audio/wav"
    }

    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        self.reads.lock().unwrap().push((offset, len));
        let start = usize::try_from(offset).unwrap();
        let end = (start + len).min(self.bytes.len());
        Ok(self.bytes.get(start..end).unwrap_or_default().to_vec())
    }
}

/// `seconds` of 8 kHz mono 16-bit silence.
fn silence(seconds: u32) -> Vec<u8> {
    let (rate, block) = (8_000u32, 2u16);
    let data = rate * seconds * u32::from(block);
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    wav.extend_from_slice(&block.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    wav.resize(wav.len() + data as usize, 0);
    wav
}

#[test]
fn closed_after_close() {
    let mut ui = Ui::new();
    let root = desktop_host::owner_safety::test_root().unwrap();
    let folder = root.path().join("data").join("webview2");
    let reads = Arc::new(Mutex::new(Vec::new()));
    let source = Arc::new(Wav {
        bytes: silence(40),
        reads: reads.clone(),
    });
    let spawner = ui.spawner();
    let spawn: Spawn = Rc::new(move |task| {
        let _ = spawner.spawn_local(task);
    });
    let (parent, bounds) = (ui.parent, ui.client_rect());
    assert_eq!(live_controllers(), 0);
    let viewer = ui
        .run(async move { MediaViewer::open(parent, bounds, source, false, &folder, spawn).await })
        .expect("the player opens");
    assert_eq!(live_controllers(), 1, "an open player is counted");
    ui.wait_until("the player reads its file", Duration::from_secs(30), || {
        !reads.lock().unwrap().is_empty()
    });
    for &(offset, len) in reads.lock().unwrap().iter() {
        assert!(
            (1..=CHUNK).contains(&len),
            "a read of {len} bytes at {offset}"
        );
    }
    viewer.close();
    assert_eq!(live_controllers(), 0, "a closed player isn't counted");
    // Its last answers find nothing to answer, and nothing panics.
    ui.settle(Duration::from_millis(500));
}
