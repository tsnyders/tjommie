//! The rules of a live stream's frames (`native-browser-frame.ts`): which of Chromium's screencast frames are forwarded,
//! how fast, and which one waits when delivery is slow. No browser and no clock of its own: everything here is plain logic
//! that the stream driver feeds and a test can drive.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use super::cdp::{Method, NoParams, Nothing, method};

/// The largest image a frame may carry, in bytes.
pub const IMAGE_LIMIT: usize = 2 * 1024 * 1024;
/// A frame wider or taller than this is not forwarded.
const MAX_SIDE: u32 = 4096;

/// What `Page.startScreencast` asks for: JPEG, quality 60, at most 2560 by 1600, every frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartScreencastParams {
    pub format: &'static str,
    pub quality: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub every_nth_frame: u32,
}

pub const SCREENCAST: StartScreencastParams = StartScreencastParams {
    format: "jpeg",
    quality: 60,
    max_width: 2560,
    max_height: 1600,
    every_nth_frame: 1,
};

/// `Page.screencastFrame`: one frame from Chromium. Chromium sends no further frame until this one is acknowledged.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreencastFrame {
    pub session_id: Option<i64>,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub metadata: FrameMetadata,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameMetadata {
    pub device_width: Option<f64>,
    pub device_height: Option<f64>,
}

/// The parameters of `Page.screencastFrameAck`: Chromium sends the next frame only after this one is acknowledged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckParams {
    pub session_id: i64,
}

method!(
    /// Starts the page's screencast.
    PageStartScreencast, "Page.startScreencast", StartScreencastParams, Nothing
);
method!(
    /// Stops the page's screencast.
    PageStopScreencast, "Page.stopScreencast", NoParams, Nothing
);
method!(
    /// Lets Chromium send the next screencast frame.
    PageScreencastFrameAck, "Page.screencastFrameAck", AckParams, Nothing
);

/// A frame that passed validation, ready to forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Standard base64 of the JPEG.
    pub data: String,
    pub width: u32,
    pub height: u32,
    pub device_width: u32,
    pub device_height: u32,
}

/// The pixel size of a JPEG, read from its start-of-frame marker without decoding the image. `None` if it has none.
pub fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes.first() != Some(&0xff) || bytes.get(1) != Some(&0xd8) {
        return None;
    }
    let at = |i: usize| bytes.get(i).copied().map(u32::from);
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        if at(i)? != 0xff {
            return None;
        }
        let marker = at(i + 1)?;
        if marker == 0xff {
            i += 1; // a fill byte before a marker
            continue;
        }
        if marker == 0x01 || (0xd0..=0xd9).contains(&marker) {
            i += 2; // markers with no segment
            continue;
        }
        let length = (at(i + 2)? << 8) | at(i + 3)?;
        if length < 2 {
            return None;
        }
        // SOF0 to SOF15, except the tables that share their range: DHT (C4), JPG (C8) and DAC (CC).
        if (0xc0..=0xcf).contains(&marker) && marker != 0xc4 && marker != 0xc8 && marker != 0xcc {
            if i + 9 > bytes.len() {
                return None;
            }
            let height = (at(i + 5)? << 8) | at(i + 6)?;
            let width = (at(i + 7)? << 8) | at(i + 8)?;
            return (width > 0 && height > 0).then_some((width, height));
        }
        i += 2 + usize::try_from(length).ok()?;
    }
    None
}

fn is_base64(text: &str) -> bool {
    let body = text.trim_end_matches('=');
    text.len() - body.len() <= 2
        && !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// The frame, if it is a sound one: base64 within 2 MiB, a JPEG whose size reads and fits 4096 by 4096. The device size
/// is the metadata's, rounded, when it is sensible (1 to 4096), else the image's own.
pub fn validate(frame: &ScreencastFrame) -> Option<Frame> {
    if frame.data.len() > IMAGE_LIMIT.div_ceil(3) * 4 || !is_base64(&frame.data) {
        return None;
    }
    // The start-of-frame marker sits in the first few hundred bytes; 65536 characters are whole base64 groups.
    let head: String = frame.data.chars().take(65536).collect();
    let (width, height) = jpeg_size(&STANDARD.decode(head.as_bytes()).ok()?)?;
    if width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let device = |value: Option<f64>, fallback: u32| match value {
        Some(v) if v.is_finite() && (1.0..=4096.0).contains(&v) => v.round() as u32,
        _ => fallback,
    };
    Some(Frame {
        data: frame.data.clone(),
        width,
        height,
        device_width: device(frame.metadata.device_width, width),
        device_height: device(frame.metadata.device_height, height),
    })
}

/// Frames per second, enforced by dropping: at most `max_fps` get through; 60 means every frame Chromium makes.
#[derive(Debug, Clone)]
pub struct Throttle {
    max_fps: u32,
    last_ms: Option<f64>,
}

impl Throttle {
    pub fn new(max_fps: u32) -> Self {
        Self {
            max_fps,
            last_ms: None,
        }
    }

    /// Takes effect from the next frame.
    pub fn set_max_fps(&mut self, max_fps: u32) {
        self.max_fps = max_fps;
    }

    /// A little under the period, so timing jitter does not turn 30 fps into 15.
    fn gap(&self) -> f64 {
        if self.max_fps >= 60 {
            0.0
        } else {
            1000.0 / f64::from(self.max_fps.max(1)) - 4.0
        }
    }

    /// Whether a frame arriving at `now_ms` is far enough after the last forwarded one.
    pub fn ready(&self, now_ms: f64) -> bool {
        !self.last_ms.is_some_and(|last| now_ms - last < self.gap())
    }

    /// A frame was forwarded at `now_ms`: the next gap starts here. (A frame that fails validation does not start one.)
    pub fn mark(&mut self, now_ms: f64) {
        self.last_ms = Some(now_ms);
    }

    /// `ready` and then `mark`: whether a frame arriving at `now_ms` is forwarded, for a frame already known to be sound.
    pub fn admit(&mut self, now_ms: f64) -> bool {
        let ready = self.ready(now_ms);
        if ready {
            self.mark(now_ms);
        }
        ready
    }
}

/// Newest frame wins: while a frame is being delivered only the newest later one waits; older ones are dropped, never
/// queued.
#[derive(Debug, Default)]
pub struct Newest {
    busy: bool,
    waiting: Option<Frame>,
}

impl Newest {
    /// A frame arrived. `Some(frame)` is to be delivered now (the caller then owns the delivery); `None` means a
    /// delivery is running and the frame waits for it.
    pub fn offer(&mut self, frame: Frame) -> Option<Frame> {
        if self.busy {
            self.waiting = Some(frame);
            None
        } else {
            self.busy = true;
            Some(frame)
        }
    }

    /// A delivery finished: the waiting frame, if any, is next; otherwise delivery is idle again.
    pub fn delivered(&mut self) -> Option<Frame> {
        let next = self.waiting.take();
        if next.is_none() {
            self.busy = false;
        }
        next
    }

    /// The stream stopped: nothing waits.
    pub fn clear(&mut self) {
        self.waiting = None;
        self.busy = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny JPEG skeleton: SOI, an APP0 segment, then a SOF0 for `width` by `height`.
    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46];
        bytes.extend([0xff, 0xc0, 0x00, 0x0b, 0x08]);
        bytes.extend(height.to_be_bytes());
        bytes.extend(width.to_be_bytes());
        bytes.extend([0x01, 0x01, 0x11, 0x00]);
        bytes
    }

    fn frame(width: u16, height: u16, metadata: FrameMetadata) -> ScreencastFrame {
        ScreencastFrame {
            session_id: Some(1),
            data: STANDARD.encode(jpeg(width, height)),
            metadata,
        }
    }

    #[test]
    fn jpeg_size_reads_the_start_of_frame_marker() {
        assert_eq!(jpeg_size(&jpeg(1280, 800)), Some((1280, 800)));
        assert_eq!(jpeg_size(&[]), None);
        assert_eq!(jpeg_size(b"not a jpeg at all"), None);
        assert_eq!(
            jpeg_size(&jpeg(1280, 800)[..10]),
            None,
            "cut off before its frame marker"
        );
        assert_eq!(
            jpeg_size(&[0xff, 0xd8, 0xff, 0xd9, 0x00, 0x00]),
            None,
            "no frame marker"
        );
        assert_eq!(jpeg_size(&jpeg(0, 800)), None, "zero dimensions");
        // A DHT segment whose payload looks like a frame marker is skipped, and the real marker found.
        let mut with_table = vec![
            0xff, 0xd8, 0xff, 0xc4, 0x00, 0x08, 0xff, 0xc0, 0x00, 0x00, 0x00, 0x00,
        ];
        with_table.extend(&jpeg(64, 32)[2..]);
        assert_eq!(jpeg_size(&with_table), Some((64, 32)));
        // Fill bytes before a marker are fine.
        let mut filled = vec![0xff, 0xd8, 0xff, 0xff];
        filled.extend(&jpeg(10, 20)[2..]);
        assert_eq!(jpeg_size(&filled), Some((10, 20)));
    }

    #[test]
    fn a_valid_frame_carries_its_size_and_a_rounded_device_size_with_fallbacks() {
        let good = validate(&frame(
            1280,
            800,
            FrameMetadata {
                device_width: Some(1279.6),
                device_height: Some(800.2),
            },
        ))
        .unwrap();
        assert_eq!(
            (
                good.width,
                good.height,
                good.device_width,
                good.device_height
            ),
            (1280, 800, 1280, 800)
        );
        let missing = validate(&frame(640, 480, FrameMetadata::default())).unwrap();
        assert_eq!((missing.device_width, missing.device_height), (640, 480));
        let silly = validate(&frame(
            640,
            480,
            FrameMetadata {
                device_width: Some(0.0),
                device_height: Some(99999.0),
            },
        ))
        .unwrap();
        assert_eq!((silly.device_width, silly.device_height), (640, 480));
    }

    #[test]
    fn an_invalid_frame_is_not_forwarded() {
        let mut bad = frame(1280, 800, FrameMetadata::default());
        bad.data = "not base64!".into();
        assert!(validate(&bad).is_none());
        assert!(
            validate(&ScreencastFrame {
                data: STANDARD.encode(b"no jpeg here"),
                ..frame(1, 1, FrameMetadata::default())
            })
            .is_none()
        );
        assert!(
            validate(&frame(5000, 800, FrameMetadata::default())).is_none(),
            "too wide"
        );
        let huge = ScreencastFrame {
            data: "A".repeat(IMAGE_LIMIT.div_ceil(3) * 4 + 4),
            ..frame(1, 1, FrameMetadata::default())
        };
        assert!(validate(&huge).is_none(), "over 2 MiB");
        assert!(
            validate(&ScreencastFrame {
                data: String::new(),
                ..frame(1, 1, FrameMetadata::default())
            })
            .is_none()
        );
    }

    #[test]
    fn the_throttle_forwards_at_most_max_fps_and_every_frame_at_60() {
        // The cases of native-browser-stream.test.ts: at 30 fps, frames at 0, 10, 20 and 33.4 ms forward the first and last.
        let mut slow = Throttle::new(30);
        assert_eq!(
            [0.0, 10.0, 20.0, 33.4].map(|at| slow.admit(at)),
            [true, false, false, true]
        );
        // At 60 fps every frame goes through, even 1 ms apart.
        let mut every = Throttle::new(60);
        assert_eq!(
            [0.0, 1.0, 2.0].map(|at| every.admit(at)),
            [true, true, true]
        );
        // A new rate applies from the next frame on.
        let mut changing = Throttle::new(60);
        assert!(changing.admit(0.0) && changing.admit(1.0));
        changing.set_max_fps(10);
        assert_eq!(
            [2.0, 50.0, 200.0].map(|at| changing.admit(at)),
            [false, false, true]
        );
    }

    #[test]
    fn a_frame_that_fails_validation_does_not_start_a_gap() {
        let mut throttle = Throttle::new(30);
        assert!(throttle.ready(0.0));
        // The frame at 0 was invalid: nothing was marked, so the next one a millisecond later still goes through.
        assert!(throttle.ready(1.0));
        throttle.mark(1.0);
        assert!(!throttle.ready(10.0) && throttle.ready(40.0));
    }

    #[test]
    fn while_a_delivery_runs_only_the_newest_later_frame_waits() {
        let f = |n: u32| Frame {
            data: n.to_string(),
            width: 1,
            height: 1,
            device_width: 1,
            device_height: 1,
        };
        let mut newest = Newest::default();
        assert_eq!(newest.offer(f(1)).map(|x| x.data), Some("1".into()));
        assert!(newest.offer(f(2)).is_none());
        assert!(newest.offer(f(3)).is_none());
        assert_eq!(
            newest.delivered().map(|x| x.data),
            Some("3".into()),
            "frame 2 was dropped, never queued"
        );
        assert!(newest.delivered().is_none());
        assert_eq!(
            newest.offer(f(4)).map(|x| x.data),
            Some("4".into()),
            "idle again"
        );
        newest.clear();
        assert!(newest.delivered().is_none());
    }
}
