//! The file viewer (`file-viewer.tsx`): a dialog titled with the file's name, with Expand or Restore and Download, over
//! the preview the file's kind has: an image (zoom 1–6×), a Markdown file by sections, PDF pages, or the media
//! player. The transfer is opened once and closed when the viewer goes, however the dialog closes. While it is open the
//! thumbnails hold back, and the one being read settles before the preview opens its transfer: the daemon allows two
//! per connection (M4 review M1).

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Image, ImageFormat, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, px,
};
use gpui_kit::component::WindowExt as _;
use protocol::attachments::{ATTACHMENT_CHUNK_BYTES, AttachmentRef};

use super::card::{self, FileFacts, FileKey, Paused};
use super::media::MediaView;
use super::pdf::PdfView;
use super::source::Opened;
use super::{PreviewKind, preview_kind};
use crate::guard::{guarded, guarded_async};
use crate::model::document_ranges::Signal;
use crate::model::file_thumbnails::{Dimensions, animated, raster_dimensions};
use crate::model::markdown_document::{MARKDOWN_SECTION_BYTES, markdown_section};
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::{IconButton, Pill, PillVariant};
use crate::ui::primitives::dialog::{open_dialog, title};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// The largest image the viewer reads whole, and the most pixels it decodes.
// ponytail: a whole-file read; an image near the 200 MiB attachment limit says "too large" instead of tiling.
const IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
const ZOOM_MAX: f32 = 6.;
const TOO_LARGE: &str = "This image is too large to preview. Download is still available.";

enum Body {
    Opening,
    Failed(SharedString),
    Image {
        image: Arc<Image>,
        width: f32,
        height: f32,
        zoom: f32,
    },
    Markdown(Entity<MarkdownFile>),
    Pdf(Entity<PdfView>),
    Media(Entity<MediaView>),
    Unsupported,
}

pub struct FileViewer {
    key: FileKey,
    reference: AttachmentRef,
    id: SharedString,
    name: SharedString,
    expanded: bool,
    body: Body,
    opened: Option<Arc<Opened>>,
    scroll: ScrollHandle,
    /// The thumbnails hold back while it's open.
    _paused: Paused,
}

/// Opens the viewer for a shared file.
pub fn open(
    key: &FileKey,
    reference: &AttachmentRef,
    facts: &FileFacts,
    window: &mut Window,
    cx: &mut App,
) {
    let kind = preview_kind(facts.content_type.as_deref().unwrap_or(""));
    let name = SharedString::from(facts.name.clone());
    let (key, reference) = (key.clone(), reference.clone());
    let viewer = cx.new(|cx| FileViewer::new(key, reference, name, kind, cx));
    open_dialog(window, cx, move |dialog, window, cx| {
        let viewer_now = viewer.read(cx);
        let room = f32::from(window.viewport_size().width);
        let width = if viewer_now.expanded {
            (room - 32.).min(1280.)
        } else {
            (room - 48.).min(960.)
        };
        let name = viewer_now.name.clone();
        dialog
            .w(px(width.max(320.)))
            .title(title(name, cx))
            .child(viewer.clone())
    });
}

impl FileViewer {
    fn new(
        key: FileKey,
        reference: AttachmentRef,
        name: SharedString,
        kind: PreviewKind,
        cx: &mut Context<Self>,
    ) -> Self {
        // However the dialog closes, the transfer is closed and the image leaves GPUI's cache once the viewer goes.
        cx.on_release(|this, cx| {
            guarded("let a file's preview go", || {
                if let Body::Image { image, .. } = &this.body {
                    image.clone().remove_asset(cx);
                }
                if let Some(opened) = this.opened.take() {
                    cx.spawn(async move |_| {
                        guarded_async("close a file's preview", opened.close()).await;
                    })
                    .detach();
                }
            });
        })
        .detach();
        let queue = card::queue(cx);
        let paused = Paused::new(queue.clone());
        let body = if kind == PreviewKind::Unsupported {
            Body::Unsupported
        } else {
            let (rpc, tokio, opening) = (store::rpc(cx), card::live_tokio(cx), reference.clone());
            cx.spawn(async move |this, cx| {
                guarded_async("open a file's preview", async move {
                    card::settled(&queue, tokio).await;
                    let opened = match Opened::open(rpc, opening).await {
                        Ok(opened) => opened,
                        Err(error) => {
                            let _ = this.update(cx, |this, cx| this.fail(error, cx));
                            return;
                        }
                    };
                    let image = if kind == PreviewKind::Image {
                        Some(read_image(&opened).await)
                    } else {
                        None
                    };
                    let closing = opened.clone();
                    let kept = this.update(cx, |this, cx| {
                        this.body = match image {
                            Some(Ok((image, width, height))) => Body::Image {
                                image,
                                width,
                                height,
                                zoom: 1.,
                            },
                            Some(Err(error)) => Body::Failed(error.into()),
                            None => match kind {
                                PreviewKind::Markdown => {
                                    let (id, file) = (this.id.clone(), opened.clone());
                                    Body::Markdown(cx.new(|cx| MarkdownFile::new(id, file, cx)))
                                }
                                PreviewKind::Pdf => {
                                    let (id, file) = (this.id.clone(), opened.clone());
                                    Body::Pdf(cx.new(|cx| PdfView::new(id, file, cx)))
                                }
                                PreviewKind::Audio | PreviewKind::Video => {
                                    let (id, file) = (this.id.clone(), opened.clone());
                                    let video = kind == PreviewKind::Video;
                                    Body::Media(cx.new(|cx| MediaView::new(id, file, video, cx)))
                                }
                                _ => Body::Unsupported,
                            },
                        };
                        this.opened = Some(opened);
                        cx.notify();
                    });
                    // Closed while it opened: nothing else will close the transfer.
                    if kept.is_err() {
                        closing.close().await;
                    }
                })
                .await;
            })
            .detach();
            Body::Opening
        };
        FileViewer {
            id: format!("{}.viewer", key.id()).into(),
            key,
            reference,
            name,
            expanded: false,
            body,
            opened: None,
            scroll: ScrollHandle::new(),
            _paused: paused,
        }
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.body = Body::Failed(error.into());
        cx.notify();
    }

    fn zoom(&mut self, by: f32, cx: &mut Context<Self>) {
        if let Body::Image { zoom, .. } = &mut self.body {
            *zoom = if by == 0. {
                1.
            } else {
                (*zoom * by).clamp(1., ZOOM_MAX)
            };
            if by == 0. {
                self.scroll.set_offset(gpui::point(px(0.), px(0.)));
            }
            cx.notify();
        }
    }
}

/// Past the viewer's own limit (`IMAGE_PIXELS`), not the thumbnail's: a picture too large to preview.
fn too_large(size: Dimensions) -> bool {
    u64::from(size.width) * u64::from(size.height) > IMAGE_PIXELS
}

/// An image, read whole within the limits, its size and frames checked before GPUI decodes it: an animated GIF or WebP
/// is never decoded, as each of its frames would be a buffer the size of the picture (M4 review C2).
async fn read_image(opened: &Opened) -> Result<(Arc<Image>, f32, f32), String> {
    const CORRUPT: &str = "This image is corrupt or unsupported. Download is still available.";
    let info = opened.info().clone();
    let format = match info.content_type.as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => return Err(CORRUPT.into()),
    };
    if info.size > IMAGE_BYTES {
        return Err("This file is too large to preview. Download is still available.".into());
    }
    // The header settles the size from the first chunk as it would from the whole file (`raster_dimensions`): a picture
    // refused for it is refused before the rest is read (M4 re-review R8).
    let first = info.size.min(u64::from(ATTACHMENT_CHUNK_BYTES));
    let mut bytes = opened.read_span(0, first).await?;
    if raster_dimensions(&bytes, &info.content_type).is_ok_and(too_large) {
        return Err(TOO_LARGE.into());
    }
    bytes.extend(opened.read_span(first, info.size).await?);
    let size = raster_dimensions(&bytes, &info.content_type).map_err(|_| CORRUPT.to_owned())?;
    if too_large(size) || animated(&bytes, &info.content_type) {
        return Err(TOO_LARGE.into());
    }
    Ok((
        Arc::new(Image::from_bytes(format, bytes)),
        size.width as f32,
        size.height as f32,
    ))
}

/// A line of text, as the viewer says what is happening.
fn note(id: String, role: Role, text: SharedString, danger: bool, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(id, role)
        .aria_label(text.clone())
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(if danger {
            colors.danger.text
        } else {
            colors.text.secondary
        }))
        .child(text)
        .into_any_element()
}

impl Render for FileViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let id = self.id.clone();
        let entity = cx.entity().downgrade();
        // The dialog's height in the TypeScript, less its padding and title: min(760, window − 48), or window − 32.
        let room = f32::from(window.viewport_size().height);
        let height = if self.expanded {
            room - 32. - 112.
        } else {
            (room - 48.).min(760.) - 112.
        };
        let actions = {
            let expand = entity.clone();
            let (key, reference) = (self.key.clone(), self.reference.clone());
            let file = self.name.to_string();
            div()
                .flex()
                .justify_end()
                .gap(r(theme.space.s8))
                .child(
                    Pill::new(
                        format!("{id}.expand"),
                        if self.expanded { "Restore" } else { "Expand" },
                    )
                    .variant(PillVariant::Grey)
                    .small()
                    .on_press(move |_, cx| {
                        let _ = expand.update(cx, |this, cx| {
                            this.expanded = !this.expanded;
                            cx.notify();
                        });
                    }),
                )
                .child(
                    Pill::new(format!("{id}.download"), "Download")
                        .small()
                        .on_press(move |window, cx| {
                            // The preview closes first (its transfer with it), then the download starts; its status
                            // shows on the file's card.
                            window.close_dialog(cx);
                            card::download(&key, &reference, &file, cx);
                        }),
                )
        };
        let body: AnyElement = match &self.body {
            Body::Opening => note(
                format!("{id}.status"),
                Role::Status,
                "Opening file…".into(),
                false,
                cx,
            ),
            Body::Failed(error) => note(
                format!("{id}.problem"),
                Role::Alert,
                error.clone(),
                true,
                cx,
            ),
            Body::Unsupported => note(
                format!("{id}.status"),
                Role::Status,
                "This file type has no preview. Use Download to save it on this device.".into(),
                false,
                cx,
            ),
            Body::Markdown(file) => file.clone().into_any_element(),
            Body::Pdf(pdf) => pdf.clone().into_any_element(),
            Body::Media(media) => media.clone().into_any_element(),
            Body::Image {
                image,
                width,
                height,
                zoom,
            } => {
                let (out, inn, reset) = (entity.clone(), entity.clone(), entity.clone());
                let zoom = *zoom;
                let picture = img(image.clone()).object_fit(ObjectFit::Contain);
                let picture = if zoom <= 1. {
                    picture.max_w_full().max_h_full()
                } else {
                    picture.w(px(width * zoom)).h(px(height * zoom))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s8))
                    .size_full()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(r(theme.space.s8))
                            .child(
                                IconButton::new(
                                    format!("{id}.zoom-out"),
                                    "minus",
                                    "Zoom image out",
                                )
                                .disabled(zoom <= 1.)
                                .on_press(move |_, cx| {
                                    let _ = out.update(cx, |this, cx| this.zoom(1. / 1.5, cx));
                                }),
                            )
                            .child(
                                IconButton::new(format!("{id}.zoom-in"), "plus", "Zoom image in")
                                    .disabled(zoom >= ZOOM_MAX)
                                    .on_press(move |_, cx| {
                                        let _ = inn.update(cx, |this, cx| this.zoom(1.5, cx));
                                    }),
                            )
                            .child(
                                Pill::new(format!("{id}.reset"), "Reset")
                                    .variant(PillVariant::Grey)
                                    .small()
                                    .on_press(move |_, cx| {
                                        let _ = reset.update(cx, |this, cx| this.zoom(0., cx));
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .semantic(format!("{id}.image"), Role::Image)
                            .aria_label(format!("{}; scroll to pan when zoomed", self.name))
                            .flex_1()
                            .min_h_0()
                            .overflow_scroll()
                            .track_scroll(&self.scroll)
                            .child(picture),
                    )
                    .into_any_element()
            }
        };
        div()
            .semantic(format!("{id}"), Role::Group)
            .aria_label(self.name.clone())
            .h(px(height.max(160.)))
            .flex()
            .flex_col()
            .gap(r(theme.space.s12))
            .child(actions)
            .child(div().flex_1().min_h_0().child(body))
    }
}

/// A Markdown file by sections of 32 KiB (`markdown-file-viewer.tsx`): Previous and Next section, the section read
/// through the transfer and drawn as replies are.
pub struct MarkdownFile {
    id: SharedString,
    opened: Arc<Opened>,
    sections: u64,
    section: u64,
    shown: Option<(u64, SharedString)>,
    problem: Option<SharedString>,
    signal: Signal,
}

impl MarkdownFile {
    fn new(id: SharedString, opened: Arc<Opened>, cx: &mut Context<Self>) -> Self {
        let sections = opened.info().size.div_ceil(MARKDOWN_SECTION_BYTES).max(1);
        let mut file = MarkdownFile {
            id,
            opened,
            sections,
            section: 0,
            shown: None,
            problem: None,
            signal: Signal::new(),
        };
        file.read(cx);
        file
    }

    fn go(&mut self, section: u64, cx: &mut Context<Self>) {
        if section < self.sections && section != self.section {
            self.section = section;
            self.read(cx);
            cx.notify();
        }
    }

    /// Reads the current section; an older read that answers late is dropped.
    fn read(&mut self, cx: &mut Context<Self>) {
        self.signal.abort();
        self.signal = Signal::new();
        let (opened, signal, section) = (self.opened.clone(), self.signal.clone(), self.section);
        let size = opened.info().size;
        cx.spawn(async move |this, cx| {
            guarded_async("read a Markdown section", async move {
                let read = markdown_section(&*opened, "", size, section, &signal).await;
                if signal.aborted() {
                    return;
                }
                // Rewritten off the UI thread, and never inside an update.
                let read = match read {
                    Ok(read) => Ok(cx
                        .background_spawn(async move { super::markdown::sanitize(&read.text) })
                        .await),
                    Err(error) => Err(error),
                };
                let _ = this.update(cx, |this, cx| {
                    match read {
                        Ok(safe) => {
                            this.shown = Some((section, safe.into()));
                            this.problem = None;
                        }
                        Err(error) => this.problem = Some(error.to_string().into()),
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
    }
}

impl Drop for MarkdownFile {
    fn drop(&mut self) {
        self.signal.abort();
    }
}

impl Render for MarkdownFile {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let colors = palette(cx);
        let id = self.id.clone();
        let entity = cx.entity().downgrade();
        let (back, next) = (entity.clone(), entity);
        let section = self.section;
        let content: AnyElement = match (&self.problem, &self.shown) {
            (Some(problem), _) => note(
                format!("{id}.problem"),
                Role::Alert,
                problem.clone(),
                true,
                cx,
            ),
            (None, Some((shown, text))) if *shown == section => super::markdown::document(
                SharedString::from(format!("{id}.section.{section}")),
                text.clone(),
                cx,
            )
            .into_any_element(),
            _ => note(
                format!("{id}.status"),
                Role::Status,
                "Reading section…".into(),
                false,
                cx,
            ),
        };
        div()
            .flex()
            .flex_col()
            .gap(r(theme.space.s8))
            .size_full()
            .when(self.sections > 1, |file| {
                file.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s8))
                        .child(
                            IconButton::new(
                                format!("{id}.previous"),
                                "chevron-left",
                                "Previous section",
                            )
                            .disabled(section == 0)
                            .on_press(move |_, cx| {
                                let _ = back
                                    .update(cx, |this, cx| this.go(section.saturating_sub(1), cx));
                            }),
                        )
                        .child(
                            div()
                                .semantic(format!("{id}.section"), Role::Status)
                                .aria_label(format!("Section {} of {}", section + 1, self.sections))
                                .type_style(&theme.typography.desktop.meta)
                                .text_color(hsla(colors.text.secondary))
                                .child(format!("Section {} / {}", section + 1, self.sections)),
                        )
                        .child(
                            IconButton::new(format!("{id}.next"), "chevron-right", "Next section")
                                .disabled(section + 1 >= self.sections)
                                .on_press(move |_, cx| {
                                    let _ = next.update(cx, |this, cx| this.go(section + 1, cx));
                                }),
                        ),
                )
                .child(note(
                    format!("{id}.sections"),
                    Role::Paragraph,
                    "Large Markdown is read in sections. A block may continue in the next section."
                        .into(),
                    false,
                    cx,
                ))
            })
            .child(
                div()
                    .id(SharedString::from(format!("{id}.document")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::files::source::testing::{asked, png, reference, serving};

    /// A 20-megapixel photo is within the viewer's own limit (64 megapixels): it is read whole and previewed. The
    /// thumbnail's 16-megapixel cap is the card's alone; the viewer once called such a photo corrupt.
    #[gpui::test]
    fn a_twenty_megapixel_photo_is_previewed(cx: &mut gpui::TestAppContext) {
        let (rpc, calls) = serving(cx, png(5472, 3648, 4 * 1024 * 1024), "image/png");
        let read = futures::executor::block_on(async {
            let opened = Opened::open(rpc, reference()).await.unwrap();
            read_image(&opened)
                .await
                .map(|(_, width, height)| (width, height))
        });
        assert_eq!(read, Ok((5472., 3648.)));
        assert!(asked(&calls, "attachments.read") > 1, "read whole");
    }

    /// M4 re-review R8: a picture past the viewer's limit is too large to preview (not corrupt), and says so once its
    /// first chunk is in: the megabytes after its header aren't asked for.
    #[gpui::test]
    fn a_picture_past_the_viewers_limit_is_too_large_from_its_first_chunk(
        cx: &mut gpui::TestAppContext,
    ) {
        let (rpc, calls) = serving(cx, png(10_000, 8_000, 4 * 1024 * 1024), "image/png");
        let refused = futures::executor::block_on(async {
            let opened = Opened::open(rpc, reference()).await.unwrap();
            read_image(&opened).await.err()
        });
        assert_eq!(refused.as_deref(), Some(TOO_LARGE));
        assert_eq!(
            asked(&calls, "attachments.read"),
            1,
            "the rest of the file was read"
        );
    }
}
