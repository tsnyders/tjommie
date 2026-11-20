//! The file viewer (`file-viewer.tsx`): a dialog titled with the file's name, with Expand or Restore and Download, over
//! the preview the file's kind has: an image (zoom 1–6×), a Markdown file by sections, PDF pages, or the media
//! player. The transfer is opened once and closed when the viewer goes, however the dialog closes.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Image, ImageFormat, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, WeakEntity, Window, div, img,
    px,
};
use gpui_kit::component::WindowExt as _;
use protocol::attachments::AttachmentRef;

use super::card::FileCard;
use super::media::MediaView;
use super::pdf::PdfView;
use super::source::Opened;
use super::{PreviewKind, preview_kind};
use crate::guard::{guarded, guarded_async};
use crate::model::document_ranges::Signal;
use crate::model::file_thumbnails::raster_dimensions;
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
    card: WeakEntity<FileCard>,
    id: SharedString,
    name: SharedString,
    expanded: bool,
    body: Body,
    opened: Option<Arc<Opened>>,
    scroll: ScrollHandle,
}

/// Opens the viewer for the file `card` shows.
pub fn open(card: &Entity<FileCard>, window: &mut Window, cx: &mut App) {
    let (facts, reference) = {
        let card = card.read(cx);
        (card.facts().clone(), card.reference().clone())
    };
    let kind = preview_kind(facts.content_type.as_deref().unwrap_or(""));
    let id = SharedString::from(format!(
        "file.{}.{}.viewer",
        reference.message_id, reference.attachment_index
    ));
    let weak = card.downgrade();
    let name = SharedString::from(facts.name);
    let viewer = cx.new(|cx| FileViewer::new(weak, id, name, kind, reference, cx));
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
        card: WeakEntity<FileCard>,
        id: SharedString,
        name: SharedString,
        kind: PreviewKind,
        reference: AttachmentRef,
        cx: &mut Context<Self>,
    ) -> Self {
        // However the dialog closes, the transfer is closed once the viewer goes.
        cx.on_release(|this, cx| {
            guarded("let a file's preview go", || {
                if let Some(opened) = this.opened.take() {
                    cx.spawn(async move |_| {
                        guarded_async("close a file's preview", opened.close()).await;
                    })
                    .detach();
                }
            });
        })
        .detach();
        let body = if kind == PreviewKind::Unsupported {
            Body::Unsupported
        } else {
            let rpc = store::rpc(cx);
            cx.spawn(async move |this, cx| {
                guarded_async("open a file's preview", async move {
                    let opened = match Opened::open(rpc, reference).await {
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
            card,
            id,
            name,
            expanded: false,
            body,
            opened: None,
            scroll: ScrollHandle::new(),
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

/// An image, read whole within the limits, its size checked from its header before GPUI decodes it.
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
    let bytes = opened.read_all(IMAGE_BYTES).await?;
    let size = raster_dimensions(&bytes, &info.content_type).map_err(|_| CORRUPT.to_owned())?;
    if u64::from(size.width) * u64::from(size.height) > IMAGE_PIXELS {
        return Err("This image is too large to preview. Download is still available.".into());
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
            let card = self.card.clone();
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
                            // The preview closes first (its transfer with it), then the card's download starts.
                            window.close_dialog(cx);
                            if let Some(card) = card.upgrade() {
                                FileCard::start_download(&card, cx);
                            }
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
                let _ = this.update(cx, |this, cx| {
                    match read {
                        Ok(read) => {
                            this.shown =
                                Some((section, super::markdown::sanitize(&read.text).into()));
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
