//! Port of `packages/ui/lib/browser-geometry.ts` (T1.4): how the browser's picture and the user's pointer relate, the address
//! bar's reading, and the key routes. Pure, so the viewport's math can be tested without a DOM.
//!
//! JavaScript numbers are `f64` here. `Math.round` halves go up (`round_half_up`). Text is read as the JavaScript code
//! units would be for the inputs the app sends (ASCII, and BMP characters), by `char`.

use protocol::browser::{BrowserFrame, BrowserState, MouseButton};
use url::Url;

/// A width and height (JavaScript numbers, CSS pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

impl From<&BrowserFrame> for Size {
    fn from(frame: &BrowserFrame) -> Self {
        Size::new(f64::from(frame.width), f64::from(frame.height))
    }
}

/// A box: where it sits and how big it is (TypeScript's `Box`, and `containedRect`'s result).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

/// A point in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// What a key event carries: the key's name and the modifiers held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent<'a> {
    pub key: &'a str,
    pub shift_key: bool,
    pub ctrl_key: bool,
    pub meta_key: bool,
}

/// What a key in the viewport does: "leave" is Shift+Tab, which returns focus to the panel's controls instead of the page;
/// "paste" waits for the paste event, which carries the text; everything else goes to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    Leave,
    Paste,
    Send,
}

/// What the picture element shows: whether it has decoded, its natural size, and its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub complete: bool,
    pub natural_width: u32,
    pub natural_height: u32,
    pub current_src: String,
}

const MIN: Size = Size::new(240.0, 180.0);
const MAX: Size = Size::new(2560.0, 1600.0);

/// `Math.round` for the values here: halves go up.
fn round_half_up(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// Where a frame is drawn inside its box with object-contain: centered, scaled to fit, the rest letterboxed.
pub fn contained_rect(boxed: Size, frame: Size) -> Rect {
    if boxed.width <= 0.0 || boxed.height <= 0.0 || frame.width <= 0.0 || frame.height <= 0.0 {
        return Rect {
            left: 0.0,
            top: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }
    let scale = (boxed.width / frame.width).min(boxed.height / frame.height);
    let width = frame.width * scale;
    let height = frame.height * scale;
    Rect {
        left: (boxed.width - width) / 2.0,
        top: (boxed.height - height) / 2.0,
        width,
        height,
    }
}

/// The page's own (CSS) size a frame shows: the frame says so when its picture was scaled, else the picture's size is it.
pub fn frame_viewport(frame: &BrowserFrame) -> Size {
    let width = frame
        .viewport_width
        .filter(|w| *w > 0)
        .unwrap_or(frame.width);
    let height = frame
        .viewport_height
        .filter(|h| *h > 0)
        .unwrap_or(frame.height);
    Size::new(f64::from(width), f64::from(height))
}

/// `Math.min(Math.max(Math.ceil(of) - 1, 0), Math.max(0, Math.floor((offset / size) * of)))`: a coordinate, kept under the
/// page's own size (the daemon rejects x >= width and y >= height).
fn at(offset: f64, size: f64, of: f64) -> f64 {
    let last = (of.ceil() - 1.0).max(0.0);
    let inside = ((offset / size) * of).floor().max(0.0);
    last.min(inside)
}

/// A pointer's place on the page, in the page's CSS pixels, or `None` when it is over the letterbox, outside the drawn
/// picture. The picture is drawn contained by its own (encoded) shape; the point is its fraction of the picture, scaled to
/// `page` (which defaults to the picture's own size).
pub fn frame_point(
    boxed: Rect,
    picture: Size,
    client_x: f64,
    client_y: f64,
    page: Option<Size>,
) -> Option<Point> {
    let page = page.unwrap_or(picture);
    let drawn = contained_rect(boxed.size(), picture);
    if drawn.width <= 0.0 || page.width <= 0.0 || page.height <= 0.0 {
        return None;
    }
    let dx = client_x - boxed.left - drawn.left;
    let dy = client_y - boxed.top - drawn.top;
    if dx < 0.0 || dy < 0.0 || dx > drawn.width || dy > drawn.height {
        return None;
    }
    Some(Point {
        x: at(dx, drawn.width, page.width),
        y: at(dy, drawn.height, page.height),
    })
}

/// Like `frame_point`, but a pointer outside the drawn picture lands on its nearest edge. For releasing a button that went
/// down inside the picture: the release must reach the page even if the pointer left it, or the page keeps it held.
pub fn frame_point_clamped(
    boxed: Rect,
    picture: Size,
    client_x: f64,
    client_y: f64,
    page: Option<Size>,
) -> Option<Point> {
    let drawn = contained_rect(boxed.size(), picture);
    if drawn.width <= 0.0 {
        return None;
    }
    let left = boxed.left + drawn.left;
    let top = boxed.top + drawn.top;
    let x = (left + drawn.width).min(left.max(client_x));
    let y = (top + drawn.height).min(top.max(client_y));
    frame_point(boxed, picture, x, y, page)
}

/// The size to ask the page for: what the viewport shows, within what the daemon accepts.
pub fn viewport_size(width: f64, height: f64) -> Size {
    Size::new(
        round_half_up(width).clamp(MIN.width, MAX.width),
        round_half_up(height).clamp(MIN.height, MAX.height),
    )
}

/// Whether a new size is worth asking for: drags and window moves produce many near-equal sizes.
pub fn resize_worth_it(last: Option<Size>, next: Size, step: f64) -> bool {
    match last {
        None => true,
        Some(last) => {
            (last.width - next.width).abs() >= step || (last.height - next.height).abs() >= step
        }
    }
}

/// The button a mouse event names, or `None` for the side buttons.
pub fn mouse_button(button: u8) -> Option<MouseButton> {
    match button {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    }
}

/// Wheel deltas in pixels: lines (mode 1) and pages (mode 2) become pixels, and the daemon's limit holds.
pub fn wheel_pixels(delta: f64, mode: u8) -> f64 {
    let pixels = match mode {
        1 => delta * 16.0,
        2 => delta * 400.0,
        _ => delta,
    };
    round_half_up(pixels).clamp(-10_000.0, 10_000.0)
}

/// `/^(https?|about|file):/i`. The first colon ends the scheme, and no scheme contains one.
fn has_scheme(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, _)| {
        matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "about" | "file"
        )
    })
}

/// `/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?([/?#]|$)/i`.
fn is_local(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .any(|host| lower.strip_prefix(host).is_some_and(port_then_end))
}

/// `rest` is empty, or starts with `/`, `?` or `#`.
fn ends_host(rest: &str) -> bool {
    rest.is_empty() || rest.starts_with(['/', '?', '#'])
}

/// What follows a local host: an optional `:port`, then the end or `/`, `?` or `#`.
fn port_then_end(rest: &str) -> bool {
    if ends_host(rest) {
        return true;
    }
    let Some(digits) = rest.strip_prefix(':') else {
        return false;
    };
    let count = digits.bytes().take_while(u8::is_ascii_digit).count();
    count > 0 && digits.get(count..).is_some_and(ends_host)
}

/// `!/\s/.test(value) && /^[^\s/]+\.[^\s/]{2,}([/?#:]|$)/`: a host with a dot, then a path, query, fragment or port (or
/// nothing). Checked over characters, so a multi-byte letter counts once, as a JavaScript code unit does in the BMP.
fn looks_like_host(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let chars: Vec<char> = value.chars().collect();
    let slash = chars.iter().position(|c| *c == '/').unwrap_or(chars.len());
    (1..slash).any(|dot| {
        chars.get(dot) == Some(&'.')
            && (dot + 3..=slash).any(|end| {
                end == chars.len()
                    || chars
                        .get(end)
                        .is_some_and(|c| matches!(c, '/' | '?' | '#' | ':'))
            })
    })
}

/// `encodeURIComponent`: every byte but the unreserved ASCII letters, digits and `-_.!~*'()` becomes `%XX`.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// What the address bar means: a full address as typed, a host with https (http for this computer), anything else a search.
pub fn address_to_url(text: &str) -> String {
    let value = text.trim();
    if has_scheme(value) {
        return value.to_owned();
    }
    if is_local(value) {
        return format!("http://{value}");
    }
    if looks_like_host(value) {
        return format!("https://{value}");
    }
    format!("https://duckduckgo.com/?q={}", encode_uri_component(value))
}

/// What a key in the viewport does (see `KeyRoute`).
pub fn key_route(event: &KeyEvent) -> KeyRoute {
    if event.key == "Tab" && event.shift_key {
        return KeyRoute::Leave;
    }
    if (event.ctrl_key || event.meta_key) && event.key.to_lowercase() == "v" {
        return KeyRoute::Paste;
    }
    KeyRoute::Send
}

/// True on Google's "couldn't sign you in" page, where the notice about automated browsers applies.
pub fn google_sign_in_rejected(url: &str) -> bool {
    let Ok(page) = Url::parse(url) else {
        return false;
    };
    page.host_str() == Some("accounts.google.com")
        && page
            .path()
            .to_ascii_lowercase()
            .contains("/signin/rejected")
}

/// Whether the picture on screen is really this frame, decoded: a press or wheel is mapped through a frame's size, so it must
/// not be taken while an older picture (or none yet) is what the user sees.
pub fn image_ready(image: &Shown, frame: &BrowserFrame, src: &str) -> bool {
    image.complete
        && image.natural_width == frame.width
        && image.natural_height == frame.height
        && image.current_src == src
}

/// What a picture is of: the browser session, page and layout (and stream, when the daemon names them). A new one remounts the
/// image, so nothing of the old layout stays on screen or answers clicks. Ordinary frames of the same layout share a key and
/// reuse the picture.
pub fn picture_key(frame: &BrowserFrame, state: &BrowserState) -> String {
    let page = frame
        .page_id
        .as_deref()
        .filter(|page| !page.is_empty())
        .or(state.active_page_id.as_deref())
        .unwrap_or("");
    let revision = frame
        .viewport_revision
        .map_or(String::new(), |revision| revision.to_string());
    let stream = frame.stream_id.as_deref().unwrap_or("");
    format!("{}|{page}|{revision}|{stream}", state.generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::browser::FrameMime;
    use serde_json::json;

    /// The TypeScript `frame` fixture: 800 x 600.
    fn frame() -> Size {
        Size::new(800.0, 600.0)
    }

    fn frame_of(width: u32, height: u32) -> BrowserFrame {
        BrowserFrame {
            data: "AAAA".into(),
            mime: FrameMime::Jpeg,
            width,
            height,
            sequence: 1,
            viewport_width: None,
            viewport_height: None,
            page_id: None,
            viewport_revision: None,
            stream_id: None,
            control_epoch: None,
            private_epoch: None,
        }
    }

    fn state(patch: serde_json::Value) -> BrowserState {
        let mut base = json!({
            "botId": "bot", "generation": "one", "control": "bot", "activePageId": "page", "tabs": [], "dialog": null
        });
        if let (Some(base), serde_json::Value::Object(patch)) = (base.as_object_mut(), patch) {
            base.extend(patch);
        }
        serde_json::from_value(base).expect("a browser state")
    }

    #[test]
    fn letterboxes_a_frame_that_is_wider_or_taller_than_its_box_and_fills_one_that_matches() {
        assert_eq!(
            contained_rect(Size::new(400.0, 300.0), frame()),
            Rect {
                left: 0.0,
                top: 0.0,
                width: 400.0,
                height: 300.0
            }
        );
        assert_eq!(
            contained_rect(Size::new(800.0, 300.0), frame()),
            Rect {
                left: 200.0,
                top: 0.0,
                width: 400.0,
                height: 300.0
            }
        );
        assert_eq!(
            contained_rect(Size::new(400.0, 600.0), frame()),
            Rect {
                left: 0.0,
                top: 150.0,
                width: 400.0,
                height: 300.0
            }
        );
        assert_eq!(contained_rect(Size::new(0.0, 10.0), frame()).width, 0.0);
    }

    // The box: drawn at 400 x 300, centered, so x 300..700.
    fn boxed() -> Rect {
        Rect {
            left: 100.0,
            top: 50.0,
            width: 800.0,
            height: 300.0,
        }
    }

    #[test]
    fn maps_a_pointer_over_the_drawn_picture_to_the_frames_pixels() {
        assert_eq!(
            frame_point(boxed(), frame(), 500.0, 200.0, None),
            Some(Point { x: 400.0, y: 300.0 })
        );
        assert_eq!(
            frame_point(boxed(), frame(), 300.0, 50.0, None),
            Some(Point { x: 0.0, y: 0.0 })
        );
    }

    #[test]
    fn ignores_the_letterbox() {
        assert_eq!(frame_point(boxed(), frame(), 150.0, 200.0, None), None);
        assert_eq!(frame_point(boxed(), frame(), 900.0, 200.0, None), None);
    }

    #[test]
    fn never_reaches_the_frames_own_width_or_height() {
        assert_eq!(
            frame_point(boxed(), frame(), 700.0, 350.0, None),
            Some(Point { x: 799.0, y: 599.0 })
        );
    }

    #[test]
    fn maps_onto_the_pages_own_size_when_the_picture_was_scaled_down() {
        let wide = Size::new(1280.0, 720.0); // the encoded picture
        let page = Size::new(1920.0, 1080.0); // the page it shows
        let big_box = Rect {
            left: 0.0,
            top: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        assert_eq!(
            frame_point(big_box, wide, 640.0, 360.0, Some(page)),
            Some(Point { x: 960.0, y: 540.0 })
        );
        assert_eq!(
            frame_point(big_box, wide, 1280.0, 720.0, Some(page)),
            Some(Point {
                x: 1919.0,
                y: 1079.0
            })
        );
    }

    #[test]
    fn has_no_point_while_there_is_no_drawn_picture() {
        let no_width = Rect {
            width: 0.0,
            ..boxed()
        };
        assert_eq!(frame_point(no_width, frame(), 5.0, 5.0, None), None);
    }

    #[test]
    fn asks_for_what_the_viewport_shows_within_the_daemons_limits() {
        assert_eq!(viewport_size(512.4, 400.0), Size::new(512.0, 400.0));
        assert_eq!(viewport_size(100.0, 50.0), Size::new(240.0, 180.0));
        assert_eq!(viewport_size(9000.0, 9000.0), Size::new(2560.0, 1600.0));
    }

    #[test]
    fn skips_sizes_within_a_step_of_the_last_one_asked_for() {
        let last = Size::new(500.0, 400.0);
        assert!(resize_worth_it(None, last, 16.0));
        assert!(!resize_worth_it(Some(last), Size::new(508.0, 410.0), 16.0));
        assert!(resize_worth_it(Some(last), Size::new(516.0, 400.0), 16.0));
    }

    #[test]
    fn names_mouse_buttons_and_ignores_the_side_buttons() {
        assert_eq!(
            [0u8, 1, 2, 3].map(mouse_button),
            [
                Some(MouseButton::Left),
                Some(MouseButton::Middle),
                Some(MouseButton::Right),
                None
            ]
        );
    }

    #[test]
    fn turns_wheel_lines_and_pages_into_pixels_within_the_daemons_limit() {
        assert_eq!(
            [
                wheel_pixels(3.0, 1),
                wheel_pixels(1.0, 2),
                wheel_pixels(-99999.0, 0)
            ],
            [48.0, 400.0, -10_000.0]
        );
    }

    #[test]
    fn reads_the_address_bar_as_an_address_a_host_this_computer_or_a_search() {
        assert_eq!(
            address_to_url("example.com/path"),
            "https://example.com/path"
        );
        assert_eq!(
            address_to_url("localhost:3000/a"),
            "http://localhost:3000/a"
        );
        assert_eq!(
            address_to_url("cheap flights"),
            "https://duckduckgo.com/?q=cheap%20flights"
        );
    }

    #[test]
    fn lets_shifttab_leave_the_page_waits_on_paste_and_sends_other_keys() {
        let plain = |key| KeyEvent {
            key,
            shift_key: false,
            ctrl_key: false,
            meta_key: false,
        };
        assert_eq!(
            key_route(&KeyEvent {
                shift_key: true,
                ..plain("Tab")
            }),
            KeyRoute::Leave
        );
        assert_eq!(
            key_route(&KeyEvent {
                ctrl_key: true,
                ..plain("V")
            }),
            KeyRoute::Paste
        );
        assert_eq!(key_route(&plain("a")), KeyRoute::Send);
    }

    #[test]
    fn is_true_only_on_googles_rejected_sign_in_page() {
        assert!(google_sign_in_rejected(
            "https://accounts.google.com/v3/signin/rejected?continue=x"
        ));
        assert!(!google_sign_in_rejected(
            "https://accounts.google.com/v3/signin/identifier"
        ));
        assert!(!google_sign_in_rejected(
            "https://evil.test/signin/rejected"
        ));
        assert!(!google_sign_in_rejected("not a url"));
    }

    #[test]
    fn is_the_page_size_a_frame_names_else_the_pictures_own() {
        assert_eq!(
            frame_viewport(&frame_of(1280, 720)),
            Size::new(1280.0, 720.0)
        );
        let scaled = BrowserFrame {
            viewport_width: Some(1920),
            viewport_height: Some(1080),
            ..frame_of(1280, 720)
        };
        assert_eq!(frame_viewport(&scaled), Size::new(1920.0, 1080.0));
    }

    #[test]
    fn puts_a_pointer_that_left_the_picture_on_its_nearest_edge_so_a_release_still_reaches_the_page()
     {
        assert_eq!(frame_point(boxed(), frame(), 150.0, 200.0, None), None);
        assert_eq!(
            frame_point_clamped(boxed(), frame(), 150.0, 200.0, None),
            Some(Point { x: 0.0, y: 300.0 })
        );
        assert_eq!(
            frame_point_clamped(boxed(), frame(), 5000.0, -40.0, None),
            Some(Point { x: 799.0, y: 0.0 })
        );
    }

    #[test]
    fn agrees_with_frame_point_inside_the_picture() {
        assert_eq!(
            frame_point_clamped(boxed(), frame(), 500.0, 200.0, None),
            frame_point(boxed(), frame(), 500.0, 200.0, None)
        );
    }

    #[test]
    fn is_ready_only_for_a_decoded_picture_of_this_frames_size_and_source() {
        let src = "data:image/jpeg;base64,AAAA";
        let f = frame_of(800, 600);
        let shown = Shown {
            complete: true,
            natural_width: 800,
            natural_height: 600,
            current_src: src.into(),
        };
        assert!(image_ready(&shown, &f, src));
        assert!(!image_ready(
            &Shown {
                complete: false,
                ..shown.clone()
            },
            &f,
            src
        ));
        assert!(!image_ready(
            &Shown {
                natural_width: 0,
                ..shown.clone()
            },
            &f,
            src
        ));
        assert!(!image_ready(
            &Shown {
                natural_width: 640,
                natural_height: 480,
                ..shown.clone()
            },
            &f,
            src
        )); // an older, smaller picture
        assert!(!image_ready(
            &Shown {
                current_src: "data:image/jpeg;base64,OLD".into(),
                ..shown.clone()
            },
            &f,
            src
        ));
    }

    #[test]
    fn changes_with_the_session_page_layout_or_stream_but_not_with_each_frame() {
        let st = state(json!({ "generation": "g1", "activePageId": "p1" }));
        let key = picture_key(&frame_of(800, 600), &st);
        assert_eq!(
            picture_key(
                &BrowserFrame {
                    sequence: 2,
                    ..frame_of(800, 600)
                },
                &st
            ),
            key
        );
        assert_ne!(
            picture_key(
                &frame_of(800, 600),
                &state(json!({ "generation": "g2", "activePageId": "p1" }))
            ),
            key
        );
        assert_ne!(
            picture_key(
                &frame_of(800, 600),
                &state(json!({ "generation": "g1", "activePageId": "p2" }))
            ),
            key
        );
        assert_ne!(
            picture_key(
                &BrowserFrame {
                    viewport_revision: Some(3),
                    ..frame_of(800, 600)
                },
                &st
            ),
            key
        );
        assert_ne!(
            picture_key(
                &BrowserFrame {
                    stream_id: Some("s2".into()),
                    ..frame_of(800, 600)
                },
                &st
            ),
            key
        );
    }

    #[test]
    fn address_bar_fallbacks_and_host_shapes() {
        assert_eq!(address_to_url(" https://a.test/x "), "https://a.test/x");
        assert_eq!(address_to_url("about:blank"), "about:blank");
        assert_eq!(address_to_url("127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(
            address_to_url("localhost.example.com"),
            "https://localhost.example.com"
        ); // a host, not this computer
        assert_eq!(address_to_url("a.b"), "https://duckduckgo.com/?q=a.b"); // a one-letter suffix isn't a host
        assert_eq!(
            address_to_url("example.com:8080"),
            "https://example.com:8080"
        );
    }
}
