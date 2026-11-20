//! A shared file in a chat (files.html; `file-card.tsx`): its thumbnail, which opens the preview, its name, kind and size,
//! Download (Cancel while it saves) and, on this PC, Show in folder. A download asks where to save, then streams the
//! file there a chunk at a time (`model::attachment_transfer::download_attachment`, polled with the live runtime
//! entered); a cancelled one leaves nothing behind. Reads need the file host: offline, the card says so.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, Image, ImageFormat, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, div, img, svg,
};
use protocol::attachments::AttachmentRef;

use super::native::SaveTo;
use super::source::Opened;
use super::{PreviewKind, preview_kind, viewer};
use crate::guard::guarded_async;
use crate::model::attachment_transfer::{
    DownloadOptions, HOST_REPLY, Retirements, SaveStatus, TransferError, attachment_error,
    download_attachment,
};
use crate::model::document_ranges::Signal;
use crate::model::file_thumbnails::{THUMBNAIL_IMAGE_BYTES, THUMBNAIL_PIXELS, raster_dimensions};
use crate::model::format::file_size;
use crate::model::state::Connection;
use crate::semantic::{SemanticDisabled as _, SemanticExt as _};
use crate::ui::primitives::button::{Handler, IconButton, pressable};
use crate::ui::primitives::focus_handle;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, ring_or_reveal, shadows, theme};

/// What a card shows of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileFacts {
    pub name: String,
    pub path: String,
    pub size: Option<u64>,
    pub content_type: Option<String>,
}

enum Thumb {
    None,
    Loading,
    Ready(Arc<Image>),
    Failed,
}

pub struct FileCard {
    id: SharedString,
    facts: FileFacts,
    reference: AttachmentRef,
    thumb: Thumb,
    saving: Option<Signal>,
    status: Option<SharedString>,
}

/// The card for attachment `index` of message `message_id`, kept while it shows.
pub fn card(
    message_id: &str,
    index: u64,
    attachment_id: Option<String>,
    facts: FileFacts,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = SharedString::from(format!("file.{message_id}.{index}"));
    let reference = AttachmentRef {
        message_id: message_id.to_owned(),
        attachment_index: index,
        attachment_id,
    };
    let key = id.clone();
    window
        .use_keyed_state(key, cx, |_, _| FileCard {
            id,
            facts,
            reference,
            thumb: Thumb::None,
            saving: None,
            status: None,
        })
        .into_any_element()
}

fn image_format(content_type: &str) -> Option<ImageFormat> {
    match content_type {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::Webp),
        _ => None,
    }
}

/// The kind's word on the card ("PDF · 2.4 MB").
pub fn kind_word(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::Markdown => "Markdown",
        PreviewKind::Image => "Image",
        PreviewKind::Pdf => "PDF",
        PreviewKind::Audio => "Audio",
        PreviewKind::Video => "Video",
        PreviewKind::Unsupported => "File",
    }
}

fn connected(cx: &App) -> bool {
    store::state(cx).connection == Connection::Ready
}

impl FileCard {
    fn kind(&self) -> PreviewKind {
        preview_kind(self.facts.content_type.as_deref().unwrap_or(""))
    }

    /// An image's thumbnail, read whole once (within the thumbnail limits) and decoded by GPUI.
    fn load_thumbnail(&mut self, cx: &mut Context<Self>) {
        let content_type = self.facts.content_type.clone().unwrap_or_default();
        let Some(format) = image_format(&content_type) else {
            return;
        };
        if !matches!(self.thumb, Thumb::None)
            || !connected(cx)
            || self
                .facts
                .size
                .is_some_and(|size| size > THUMBNAIL_IMAGE_BYTES)
        {
            return;
        }
        self.thumb = Thumb::Loading;
        let (rpc, reference) = (store::rpc(cx), self.reference.clone());
        cx.spawn(async move |this, cx| {
            guarded_async("read a thumbnail", async move {
                let read = async {
                    let opened = Opened::open(rpc, reference).await?;
                    let bytes = opened.read_all(THUMBNAIL_IMAGE_BYTES).await;
                    opened.close().await;
                    let bytes = bytes?;
                    // The header first: a decoder never sees an image too large to draw small.
                    let size = raster_dimensions(&bytes, &content_type)
                        .map_err(|error| error.to_string())?;
                    if u64::from(size.width) * u64::from(size.height) > THUMBNAIL_PIXELS {
                        return Err("too large".to_owned());
                    }
                    Ok(Arc::new(Image::from_bytes(format, bytes)))
                };
                let outcome: Result<Arc<Image>, String> = read.await;
                let _ = this.update(cx, |this, cx| {
                    this.thumb = match outcome {
                        Ok(image) => Thumb::Ready(image),
                        Err(_) => Thumb::Failed,
                    };
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
    }

    /// Download, or Cancel while one saves.
    fn download(&mut self, cx: &mut Context<Self>) {
        if let Some(signal) = &self.saving {
            signal.abort();
            return;
        }
        if !connected(cx) {
            self.status = Some("The file host is offline. Reconnect to download it.".into());
            cx.notify();
            return;
        }
        let directory = std::env::var_os("USERPROFILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Downloads");
        let picked = cx.prompt_for_new_path(&directory, Some(&self.facts.name));
        let signal = Signal::new();
        self.saving = Some(signal.clone());
        self.status = Some("Choose where to save…".into());
        let tokio = cx
            .try_global::<crate::live::Tokio>()
            .map(|tokio| tokio.0.clone());
        let (rpc, reference) = (store::rpc(cx), self.reference.clone());
        cx.spawn(async move |this, cx| {
            guarded_async("download a file", async move {
                let Some(path) = picked.await.ok().and_then(Result::ok).flatten() else {
                    let _ = this.update(cx, |this, cx| {
                        this.saving = None;
                        this.status = Some("Download cancelled.".into());
                        cx.notify();
                    });
                    return;
                };
                let _ = this.update(cx, |this, cx| {
                    this.status = Some("Downloading 0%".into());
                    cx.notify();
                });
                let native = SaveTo::new(path);
                let (progress_this, mut progress_cx) = (this.clone(), cx.clone());
                let progress = move |done: u64, total: u64| {
                    let percent = (done * 100).checked_div(total).unwrap_or(100);
                    let _ = progress_this.update(&mut progress_cx, |this, cx| {
                        this.status = Some(format!("Downloading {percent}%").into());
                        cx.notify();
                    });
                };
                let download = async {
                    let mut retire: Retirements = Vec::new();
                    let options = DownloadOptions {
                        host_reply: HOST_REPLY,
                        reacquire: None,
                        transferring: None,
                    };
                    let outcome = download_attachment(
                        &rpc,
                        &reference,
                        &native,
                        &signal,
                        options,
                        &mut retire,
                        progress,
                    )
                    .await;
                    for late in retire {
                        late.await;
                    }
                    outcome
                };
                let outcome = crate::live::in_runtime(tokio, download).await;
                let _ = this.update(cx, |this, cx| {
                    this.saving = None;
                    this.status = Some(
                        match outcome {
                            Ok(SaveStatus::Saved) => "Saved on this device.".to_owned(),
                            Ok(SaveStatus::Cancelled) | Err(TransferError::Cancelled) => {
                                "Download cancelled.".to_owned()
                            }
                            Err(error) => match attachment_error(&error) {
                                "" => error.to_string(),
                                words => words.to_owned(),
                            },
                        }
                        .into(),
                    );
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
        cx.notify();
    }
}

impl Render for FileCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_thumbnail(cx);
        let online = connected(cx);
        let local = store::state(cx).host.is_none();
        let kind = self.kind();
        let id = self.id.clone();
        // The preview opens the viewer, by click or by Enter or Space; offline or mid-download it's unavailable.
        let saving = self.saving.is_some();
        let available = online && !saving;
        let focus = focus_handle(&format!("{id}.preview"), window, cx).tab_stop(available);
        let theme = theme(cx);
        let colors = palette(cx);
        let name = SharedString::from(self.facts.name.clone());
        let meta = match self.facts.size {
            Some(size) => format!("{} · {}", kind_word(kind), file_size(size)),
            None => kind_word(kind).to_owned(),
        };
        let icon = match kind {
            PreviewKind::Markdown => "file-text",
            PreviewKind::Image => "file-type",
            PreviewKind::Pdf => "file-text",
            PreviewKind::Audio | PreviewKind::Video => "play",
            PreviewKind::Unsupported => "file",
        };
        let picture: AnyElement = match &self.thumb {
            Thumb::Ready(image) => img(image.clone())
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            _ => svg()
                .path(format!("icons/i/175/{icon}.svg"))
                .size(r(24.))
                .text_color(hsla(colors.icon.muted))
                .into_any_element(),
        };
        let entity = cx.entity().downgrade();
        let opener = cx.entity();
        let (path, show) = (self.facts.path.clone(), local);
        let preview = div()
            .semantic(format!("{id}.preview"), Role::Button)
            .aria_label(SharedString::from(format!("Preview {name}")))
            .semantic_disabled(!available)
            .track_focus(&focus)
            .relative()
            .h(r(132.))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(r(theme.radius.sm))
            .bg(hsla(colors.surface.panel))
            .when(available, |preview| preview.cursor_pointer())
            .when(!available, |preview| preview.opacity(0.5))
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .rounded(r(theme.radius.sm))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(picture),
            )
            .children(ring_or_reveal(&focus, theme.radius.sm, window, cx));
        let preview = pressable(
            preview,
            available.then(|| {
                std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                    viewer::open(&opener, window, cx);
                }) as Handler
            }),
        );
        div()
            .semantic(id.clone(), Role::Group)
            .aria_label(name.clone())
            .w(r(264.))
            .max_w_full()
            .flex()
            .flex_col()
            .gap(r(theme.space.s8))
            .p(r(theme.space.s12))
            .bg(hsla(colors.surface.raised))
            .rounded(r(theme.radius.md))
            .border_t_1()
            .border_color(hsla(colors.overlay.highlight))
            .shadow(shadows(&theme.shadow.light.card))
            .child(preview)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s4))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .line_clamp(2)
                                    .type_style(&theme.typography.desktop.ui_strong)
                                    .text_color(hsla(colors.text.primary))
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .type_style(&theme.typography.desktop.meta)
                                    .text_color(hsla(colors.text.tertiary))
                                    .child(meta),
                            ),
                    )
                    .child(
                        IconButton::new(
                            format!("{id}.download"),
                            if saving { "x" } else { "download" },
                            if saving {
                                format!("Cancel download of {name}")
                            } else {
                                format!("Download {name}")
                            },
                        )
                        .on_press(move |_, cx| {
                            let _ = entity.update(cx, |this, cx| this.download(cx));
                        }),
                    )
                    .when(show, |row| {
                        row.child(
                            IconButton::new(
                                format!("{id}.folder"),
                                "folder",
                                format!("Show {name} in its folder"),
                            )
                            .on_press(move |_, _| {
                                let _ = desktop_host::links::show_in_folder(&path);
                            }),
                        )
                    }),
            )
            .when_some(self.status.clone(), |card, status| {
                card.child(
                    div()
                        .semantic(format!("{id}.status"), Role::Status)
                        .aria_label(status.clone())
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.secondary))
                        .child(status),
                )
            })
    }
}

impl FileCard {
    pub fn facts(&self) -> &FileFacts {
        &self.facts
    }

    pub fn reference(&self) -> &AttachmentRef {
        &self.reference
    }

    /// The viewer's Download: the card's own (its status shows on the card).
    pub fn start_download(entity: &Entity<FileCard>, cx: &mut App) {
        entity.update(cx, |this, cx| this.download(cx));
    }
}
