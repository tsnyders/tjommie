//! Another PC's page as a picture (Phase 3 plan T3.7; browser-panel.tsx `Viewport`): the live stream's frames
//! (`browser.streamFrame`), or the polled `browser.frame` on a daemon without the stream, drawn contained in the page's
//! place. A new frame is decoded while the last one stays on screen; once it is drawn, the session is told (`Drawn`), so
//! the stream sends the next: the daemon holds at most two unacknowledged frames, so the viewer paces the stream. A
//! private interaction shows no picture at all, whatever reaches the panel.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine as _;
use gpui::{
    AnyElement, App, Image, ImageFormat, ImageSource, IntoElement, ObjectFit, ParentElement as _,
    RenderImage, Styled as _, StyledImage as _, Window, canvas, div, img,
};
use protocol::browser::BrowserFrame;

use crate::guard::guarded;

/// A frame and its picture.
struct Shown {
    frame: BrowserFrame,
    image: Arc<Image>,
    /// The decoded picture, once GPUI has it.
    render: Option<Arc<RenderImage>>,
    /// The session has been told it was drawn.
    reported: Rc<Cell<bool>>,
}

/// The picture a remote panel shows, kept across frames.
#[derive(Default)]
pub struct RemotePicture {
    current: Option<Shown>,
    /// The newest frame, being decoded.
    pending: Option<Shown>,
}

/// Whether two frames are the same picture: the same stream, sequence and page layout.
fn same(a: &BrowserFrame, b: &BrowserFrame) -> bool {
    a.sequence == b.sequence
        && a.stream_id == b.stream_id
        && a.page_id == b.page_id
        && a.viewport_revision == b.viewport_revision
        && a.data.len() == b.data.len()
}

impl RemotePicture {
    /// Takes the view's frame (none while private): a new one starts decoding, and replaces the picture on screen once
    /// it is ready.
    pub fn take(
        &mut self,
        frame: Option<&BrowserFrame>,
        private: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let frame = frame.filter(|_| !private);
        let Some(frame) = frame else {
            self.clear(window, cx);
            return;
        };
        let known = [&self.current, &self.pending]
            .into_iter()
            .flatten()
            .any(|shown| same(&shown.frame, frame));
        if !known {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&frame.data) else {
                log::warn!("A browser frame that isn't base64 was dropped");
                return;
            };
            if let Some(dropped) = self.pending.take() {
                forget(dropped, window, cx);
            }
            self.pending = Some(Shown {
                frame: frame.clone(),
                image: Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes)),
                render: None,
                reported: Rc::default(),
            });
        }
        let decoded = self
            .pending
            .as_ref()
            .and_then(|pending| pending.image.clone().use_render_image(window, cx));
        if let (Some(render), Some(mut pending)) = (decoded, self.pending.take()) {
            pending.render = Some(render);
            if let Some(old) = self.current.replace(pending) {
                forget(old, window, cx);
            }
        }
    }

    /// Drops every picture (private, or no frame).
    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        for shown in [self.current.take(), self.pending.take()]
            .into_iter()
            .flatten()
        {
            forget(shown, window, cx);
        }
    }

    /// The frame on screen, if any.
    pub fn frame(&self) -> Option<&BrowserFrame> {
        self.current.as_ref().map(|shown| &shown.frame)
    }

    /// The picture, letterboxed in its place, telling `drawn` once per frame after it is painted.
    pub fn element(
        &self,
        drawn: impl Fn(BrowserFrame, &mut Window, &mut App) + 'static,
    ) -> Option<AnyElement> {
        let shown = self.current.as_ref()?;
        let render = shown.render.clone()?;
        let (frame, reported) = (shown.frame.clone(), shown.reported.clone());
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    img(ImageSource::Render(render))
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, cx| {
                            if !reported.replace(true) {
                                let frame = frame.clone();
                                let drawn = Rc::new(drawn);
                                window.defer(cx, move |window, cx| {
                                    guarded("report a drawn frame", || drawn(frame, window, cx));
                                });
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                .into_any_element(),
        )
    }
}

/// Lets GPUI forget a picture no longer shown: its decoded asset and its place in the atlas.
fn forget(shown: Shown, window: &mut Window, cx: &mut App) {
    if let Some(render) = shown.render {
        let _ = window.drop_image(render);
    }
    shown.image.remove_asset(cx);
}
