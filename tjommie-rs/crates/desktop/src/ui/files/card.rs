//! A shared file in a chat or room (files.html; `file-card.tsx`): its thumbnail, which opens the preview, its name, kind
//! and size, Download (Cancel while it saves) and, on this PC, Show in folder. A download asks where to save, then
//! streams the file there a chunk at a time (`model::attachment_transfer::download_attachment`, polled with the live
//! runtime entered); a cancelled one leaves nothing behind. Reads need the file host: offline, the card says so.
//!
//! What a card knows lives app-wide in [`Files`], keyed by the attachment, not in the card: a thread is a virtualized
//! list that drops a card the moment it scrolls away, and its thumbnail and a download in flight must outlive that (M4
//! review M3). Thumbnails are read one at a time through the model's queue, paused while a file is previewed or saved,
//! as the daemon allows two transfers per connection (M1); each is decoded once off the UI thread and kept small, a few
//! dozen at most, and a forgotten one leaves GPUI's image cache (M2). An animated GIF or WebP is never decoded (C2).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Global, Image, ImageFormat, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, svg,
};
use protocol::attachments::AttachmentRef;

use super::native::{SaveTo, safe_file_name};
use super::source::Opened;
use super::{PreviewKind, preview_kind, viewer};
use crate::guard::guarded_async;
use crate::model::attachment_transfer::{
    DownloadOptions, HOST_REPLY, Retirements, SaveStatus, TransferError, attachment_error,
    download_attachment,
};
use crate::model::document_ranges::Signal;
use crate::model::file_thumbnails::{
    THUMBNAIL_IMAGE_BYTES, ThumbnailError, ThumbnailQueue, animated, thumbnail_dimensions,
};
use crate::model::format::file_size;
use crate::model::state::Connection;
use crate::semantic::{SemanticDisabled as _, SemanticExt as _};
use crate::ui::primitives::button::{Handler, IconButton, pressable};
use crate::ui::primitives::focus_handle;
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, ring_or_reveal, shadows, theme};

/// How many thumbnails are kept: the oldest beyond it is forgotten, and read again if its card comes back.
const THUMBNAILS: usize = 48;
/// The largest thumbnail kept, in pixels: twice the card's picture, for a high-DPI screen.
const THUMB_WIDTH: u32 = 528;
const THUMB_HEIGHT: u32 = 264;

/// What a card shows of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileFacts {
    pub name: String,
    pub path: String,
    pub size: Option<u64>,
    pub content_type: Option<String>,
}

/// An attachment, as the app keeps what it knows of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub message_id: String,
    pub index: u64,
}

impl FileKey {
    /// Its cards' and viewer's id prefix.
    pub fn id(&self) -> String {
        format!("file.{}.{}", self.message_id, self.index)
    }
}

enum Thumb {
    Loading,
    Ready(Arc<Image>),
    /// Didn't come: asked for again after a reconnect.
    Failed,
    /// Too large or animated: the file stays so, so it isn't read again (M4 re-review R6).
    Refused,
}

#[derive(Default)]
struct Save {
    saving: Option<Signal>,
    status: Option<SharedString>,
}

/// What the app knows of shared files: their thumbnails (the newest last), their downloads, and the queue thumbnails
/// are read through.
pub struct Files {
    queue: Arc<ThumbnailQueue>,
    thumbs: Vec<(FileKey, Thumb)>,
    saves: HashMap<FileKey, Save>,
    /// The connection was ready when last seen: a failed thumbnail is asked for again when it next is.
    ready: bool,
}

impl Global for Files {}

impl Default for Files {
    fn default() -> Self {
        Files {
            // As the TypeScript's queue: a thumbnail may take 10 s; a preview waits up to 30 s for one to settle.
            queue: Arc::new(ThumbnailQueue::new(
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(30),
            )),
            thumbs: Vec::new(),
            saves: HashMap::new(),
            ready: false,
        }
    }
}

impl Files {
    fn thumb(&mut self, key: &FileKey) -> Option<&Thumb> {
        let at = self.thumbs.iter().position(|(known, _)| known == key)?;
        // Seen again: the newest.
        let entry = self.thumbs.remove(at);
        self.thumbs.push(entry);
        self.thumbs.last().map(|(_, thumb)| thumb)
    }

    /// Keeps `thumb` for `key`, forgetting the oldest beyond [`THUMBNAILS`]: the pictures forgotten or replaced, for the
    /// caller to take out of GPUI's cache (M4 re-review R10).
    fn put(&mut self, key: FileKey, thumb: Thumb) -> Vec<Arc<Image>> {
        let mut forgotten = Vec::new();
        let picture = |(_, thumb): (FileKey, Thumb)| match thumb {
            Thumb::Ready(image) => Some(image),
            _ => None,
        };
        if let Some(at) = self.thumbs.iter().position(|(known, _)| *known == key) {
            forgotten.extend(picture(self.thumbs.remove(at)));
        }
        self.thumbs.push((key, thumb));
        while self.thumbs.len() > THUMBNAILS {
            forgotten.extend(picture(self.thumbs.remove(0)));
        }
        forgotten
    }
}

/// Keeps `thumb` for `key`; what it pushes out leaves GPUI's image cache (M4 review M2).
fn keep(key: FileKey, thumb: Thumb, cx: &mut App) {
    for image in cx.default_global::<Files>().put(key, thumb) {
        image.remove_asset(cx);
    }
}

/// The thumbnails' queue, for the viewer and downloads to pause.
pub fn queue(cx: &mut App) -> Arc<ThumbnailQueue> {
    cx.default_global::<Files>().queue.clone()
}

/// Holds the thumbnails back while a file is previewed or saved, until dropped.
pub struct Paused(Arc<ThumbnailQueue>);

impl Paused {
    pub fn new(queue: Arc<ThumbnailQueue>) -> Self {
        queue.pause();
        Paused(queue)
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Waits for the thumbnail being read to settle (its transfer closed), with the live runtime entered for the queue's
/// timer. A thumbnail that doesn't settle in time doesn't hold the caller back.
pub async fn settled(queue: &ThumbnailQueue, tokio: Option<tokio::runtime::Handle>) {
    // The queue waits on a Tokio timer: with no live runtime (a view test without one) there are no thumbnails to wait
    // for either.
    if tokio.is_some() {
        let _ = crate::live::in_runtime(tokio, queue.ready(&Signal::new())).await;
    }
}

pub(crate) fn live_tokio(cx: &App) -> Option<tokio::runtime::Handle> {
    cx.try_global::<crate::live::Tokio>()
        .map(|tokio| tokio.0.clone())
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

/// A file's bytes for its thumbnail: within the byte cap, the transfer closed after; the queue's signal stops it between
/// chunks, and a picture too large to draw small is refused once its first chunk is in (M4 re-review R8).
async fn read_image(
    reference: AttachmentRef,
    content_type: String,
    rpc: store::StoreRpc,
    running: Signal,
) -> Result<Vec<u8>, ThumbnailError> {
    let opened = Opened::open(rpc, reference)
        .await
        .map_err(ThumbnailError::Decode)?;
    let size = opened.info().size;
    let read = async {
        if size > THUMBNAIL_IMAGE_BYTES {
            return Err(ThumbnailError::Limit(
                "This image is too large for a thumbnail.",
            ));
        }
        let mut bytes = Vec::new();
        while (bytes.len() as u64) < size {
            if running.aborted() {
                return Err(ThumbnailError::Cancelled);
            }
            let at = bytes.len() as u64;
            let chunk = opened
                .read_span(at, (at + 256 * 1024).min(size))
                .await
                .map_err(ThumbnailError::Decode)?;
            if chunk.is_empty() {
                break;
            }
            bytes.extend(chunk);
            // The header settles the size from the first chunk as it would from the whole file (`thumbnail_dimensions`).
            if at == 0
                && let Err(refused @ ThumbnailError::Limit(_)) =
                    thumbnail_dimensions(&bytes, &content_type)
            {
                return Err(refused);
            }
        }
        Ok(bytes)
    };
    let bytes = read.await;
    opened.close().await;
    let bytes = bytes?;
    // The header first, and the frames: no decoder sees a picture too large to draw small, or an animation.
    thumbnail_dimensions(&bytes, &content_type)?;
    if animated(&bytes, &content_type) {
        return Err(ThumbnailError::Limit(
            "This image is too large for a thumbnail.",
        ));
    }
    Ok(bytes)
}

/// Decodes `bytes` once, off the UI thread, and keeps a copy at most [`THUMB_WIDTH`] by [`THUMB_HEIGHT`], its shape
/// kept and drawn over white where it's transparent: a card holds a few hundred KiB, not the whole picture (M4 review
/// M2).
fn shrink(bytes: Vec<u8>, format: ImageFormat) -> Result<Arc<Image>, String> {
    let decoded = Image::from_bytes(format, bytes)
        .to_image_data(gpui::SvgRenderer::new(Arc::new(crate::assets::Assets)))
        .map_err(|error| error.to_string())?;
    let size = decoded.size(0);
    let width = u32::try_from(size.width.0).map_err(|error| error.to_string())?;
    let height = u32::try_from(size.height.0).map_err(|error| error.to_string())?;
    let pixels = decoded.as_bytes(0).ok_or("the image has no frame")?;
    if width == 0 || height == 0 || pixels.len() < width as usize * height as usize * 4 {
        return Err("the image's frame is short".into());
    }
    let scale = (THUMB_WIDTH as f32 / width as f32)
        .min(THUMB_HEIGHT as f32 / height as f32)
        .min(1.);
    let small_width = ((width as f32 * scale).round() as u32).max(1);
    let small_height = ((height as f32 * scale).round() as u32).max(1);
    let mut small = Vec::with_capacity(small_width as usize * small_height as usize * 4);
    // The source pixels small pixel `at` of `count` stands for, of `total`: at least one.
    let span = |at: u32, count: u32, total: u32| {
        let start = u64::from(at) * u64::from(total) / u64::from(count);
        let end = (u64::from(at + 1) * u64::from(total) / u64::from(count)).max(start + 1);
        (start as usize, end.min(u64::from(total)) as usize)
    };
    // Each pixel the mean of the box of pixels it stands for.
    for y in 0..small_height {
        let (top, bottom) = span(y, small_height, height);
        for x in 0..small_width {
            let (left, right) = span(x, small_width, width);
            let mut sums = [0u64; 3];
            let mut count = 0u64;
            for row in top..bottom {
                for column in left..right {
                    let at = (row * width as usize + column) * 4;
                    if let Some(&[blue, green, red, alpha]) = pixels
                        .get(at..at + 4)
                        .and_then(|px| <&[u8; 4]>::try_from(px).ok())
                    {
                        let alpha = u64::from(alpha);
                        for (sum, channel) in sums.iter_mut().zip([blue, green, red]) {
                            *sum += (u64::from(channel) * alpha + 255 * (255 - alpha)) / 255;
                        }
                        count += 1;
                    }
                }
            }
            let count = count.max(1);
            small.extend(sums.map(|sum| (sum / count) as u8));
            small.push(255);
        }
    }
    let bmp =
        super::pdf::bmp(small_width, small_height, &small).ok_or("the thumbnail didn't encode")?;
    Ok(Arc::new(Image::from_bytes(ImageFormat::Bmp, bmp)))
}

/// After a reconnect, the thumbnails that failed are asked for again; those refused stay refused.
fn follow_connection(cx: &mut App) {
    let ready = connected(cx);
    let files = cx.default_global::<Files>();
    if ready && !files.ready {
        files
            .thumbs
            .retain(|(_, thumb)| !matches!(thumb, Thumb::Failed));
    }
    files.ready = ready;
}

/// Starts reading `key`'s thumbnail, unless it's known, the connection is down or the queue is paused (it's asked for
/// again when its card next draws).
fn load_thumbnail(key: &FileKey, reference: &AttachmentRef, facts: &FileFacts, cx: &mut App) {
    let content_type = facts.content_type.clone().unwrap_or_default();
    let Some(format) = image_format(&content_type) else {
        return;
    };
    // The queue runs on the live runtime (its timers are Tokio's); without one, no thumbnails.
    if !connected(cx)
        || facts.size.is_some_and(|size| size > THUMBNAIL_IMAGE_BYTES)
        || live_tokio(cx).is_none()
    {
        return;
    }
    let files = cx.default_global::<Files>();
    if files.queue.paused() || files.thumbs.iter().any(|(known, _)| known == key) {
        return;
    }
    let queue = files.queue.clone();
    keep(key.clone(), Thumb::Loading, cx);
    let (key, reference, rpc, tokio) = (
        key.clone(),
        reference.clone(),
        store::rpc(cx),
        live_tokio(cx),
    );
    cx.spawn(async move |cx| {
        guarded_async("read a thumbnail", async move {
            let signal = Signal::new();
            let read = crate::live::in_runtime(
                tokio,
                queue.queue(&signal, move |running| {
                    read_image(reference, content_type, rpc, running)
                }),
            )
            .await;
            let made = match read {
                Ok(bytes) => match cx
                    .background_spawn(async move { shrink(bytes, format) })
                    .await
                {
                    Ok(image) => Some(Thumb::Ready(image)),
                    Err(error) => {
                        log::debug!("A thumbnail wasn't drawn: {error}");
                        Some(Thumb::Failed)
                    }
                },
                // Stopped for a preview or a download: asked for again later.
                Err(ThumbnailError::Paused | ThumbnailError::Cancelled) => None,
                Err(ThumbnailError::Limit(why)) => {
                    log::debug!("No thumbnail: {why}");
                    Some(Thumb::Refused)
                }
                Err(error) => {
                    log::debug!("A thumbnail wasn't drawn: {error}");
                    Some(Thumb::Failed)
                }
            };
            cx.update(|cx| {
                match made {
                    Some(thumb) => keep(key, thumb, cx),
                    None => cx
                        .default_global::<Files>()
                        .thumbs
                        .retain(|(known, _)| *known != key),
                }
                cx.refresh_windows();
            });
        })
        .await;
    })
    .detach();
}

fn set_save(key: &FileKey, saving: Option<Signal>, status: Option<SharedString>, cx: &mut App) {
    let save = cx
        .default_global::<Files>()
        .saves
        .entry(key.clone())
        .or_default();
    save.saving = saving;
    save.status = status;
    cx.refresh_windows();
}

/// Settings, Files, Clear temporary data: stops every download that hasn't finished, as the download button's Cancel
/// does (each then says it was cancelled). Saved files stay. (A preview is a dialog, so none is open while that button
/// can be pressed.)
pub fn cancel_downloads(cx: &mut App) {
    for save in cx.default_global::<Files>().saves.values() {
        if let Some(signal) = &save.saving {
            signal.abort();
        }
    }
}

/// For a view test elsewhere: a download of `key` in progress, stopped through `signal`.
#[cfg(test)]
pub(crate) fn saving_for_test(key: &FileKey, signal: Signal, cx: &mut App) {
    set_save(key, Some(signal), None, cx);
}

/// Download, or Cancel while one saves: the card's, and the viewer's. The save dialog suggests the file's name, made
/// safe for Windows; thumbnails hold back until the download ends.
pub fn download(key: &FileKey, reference: &AttachmentRef, name: &str, cx: &mut App) {
    if let Some(signal) = cx
        .default_global::<Files>()
        .saves
        .get(key)
        .and_then(|save| save.saving.clone())
    {
        signal.abort();
        return;
    }
    if !connected(cx) {
        let offline = "The file host is offline. Reconnect to download it.";
        set_save(key, None, Some(offline.into()), cx);
        return;
    }
    let directory = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join("Downloads");
    let (suggested, shortened) = safe_file_name(name);
    let picked = crate::ui::prompt_for_new_path(&directory, Some(&suggested), cx);
    let signal = Signal::new();
    let choose = if shortened {
        "Choose where to save… Its name was shortened to fit Windows."
    } else {
        "Choose where to save…"
    };
    set_save(key, Some(signal.clone()), Some(choose.into()), cx);
    let (queue, tokio, rpc) = (queue(cx), live_tokio(cx), store::rpc(cx));
    let (key, reference) = (key.clone(), reference.clone());
    cx.spawn(async move |cx| {
        guarded_async("download a file", async move {
            // A dialog that couldn't open says so: only the person's own Cancel is a cancelled download (M4
            // re-review R3).
            let path = match picked.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    cx.update(|cx| set_save(&key, None, Some("Download cancelled.".into()), cx));
                    return;
                }
                failed => {
                    if let Ok(Err(error)) = failed {
                        log::warn!("The save dialog failed: {error:#}");
                    }
                    let said = "Windows couldn't show the save dialog. Try again.";
                    cx.update(|cx| set_save(&key, None, Some(said.into()), cx));
                    return;
                }
            };
            // The daemon allows two transfers per connection: the thumbnails hold back, and the one reading settles.
            let _paused = Paused::new(queue.clone());
            settled(&queue, tokio.clone()).await;
            let shown = signal.clone();
            cx.update(|cx| set_save(&key, Some(shown), Some("Downloading 0%".into()), cx));
            let native = SaveTo::new(path);
            let (progress_key, progress_cx, mut last) = (key.clone(), cx.clone(), None);
            let progress = move |done: u64, total: u64| {
                let percent = (done * 100).checked_div(total).unwrap_or(100);
                if last != Some(percent) {
                    last = Some(percent);
                    let (key, status) = (progress_key.clone(), format!("Downloading {percent}%"));
                    progress_cx.update(|cx| {
                        let save = cx.default_global::<Files>().saves.entry(key).or_default();
                        save.status = Some(status.into());
                        cx.refresh_windows();
                    });
                }
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
            let status = match outcome {
                Ok(SaveStatus::Saved) => "Saved on this device.".to_owned(),
                Ok(SaveStatus::Cancelled) | Err(TransferError::Cancelled) => {
                    "Download cancelled.".to_owned()
                }
                Err(error) => match attachment_error(&error) {
                    "" => error.to_string(),
                    words => words.to_owned(),
                },
            };
            cx.update(|cx| set_save(&key, None, Some(status.into()), cx));
        })
        .await;
    })
    .detach();
}

/// The card for attachment `index` of message `message_id`.
pub fn card(
    message_id: &str,
    index: u64,
    attachment_id: Option<String>,
    facts: FileFacts,
) -> AnyElement {
    FileCard {
        key: FileKey {
            message_id: message_id.to_owned(),
            index,
        },
        reference: AttachmentRef {
            message_id: message_id.to_owned(),
            attachment_index: index,
            attachment_id,
        },
        facts,
    }
    .into_any_element()
}

/// A file card, drawn each frame from the message (so its facts are current) and from [`Files`].
#[derive(IntoElement)]
pub struct FileCard {
    key: FileKey,
    reference: AttachmentRef,
    facts: FileFacts,
}

impl RenderOnce for FileCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        follow_connection(cx);
        load_thumbnail(&self.key, &self.reference, &self.facts, cx);
        let online = connected(cx);
        let local = store::state(cx).host.is_none();
        let kind = preview_kind(self.facts.content_type.as_deref().unwrap_or(""));
        let id = self.key.id();
        let files = cx.default_global::<Files>();
        let thumbnail = match files.thumb(&self.key) {
            Some(Thumb::Ready(image)) => Some(image.clone()),
            _ => None,
        };
        let (saving, status) = files
            .saves
            .get(&self.key)
            .map(|save| (save.saving.is_some(), save.status.clone()))
            .unwrap_or_default();
        // The preview opens the viewer, by click or by Enter or Space; offline or mid-download it's unavailable.
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
            PreviewKind::Markdown | PreviewKind::Pdf => "file-text",
            PreviewKind::Image => "file-type",
            PreviewKind::Audio | PreviewKind::Video => "play",
            PreviewKind::Unsupported => "file",
        };
        let picture: AnyElement = match thumbnail {
            Some(image) => div()
                .semantic(format!("{id}.thumbnail"), Role::Image)
                .aria_label(format!("Thumbnail of {name}"))
                .size_full()
                .child(img(image).size_full().object_fit(ObjectFit::Cover))
                .into_any_element(),
            None => svg()
                .path(format!("icons/i/175/{icon}.svg"))
                .size(r(24.))
                .text_color(hsla(colors.icon.muted))
                .into_any_element(),
        };
        let preview_name: SharedString = if available {
            format!("Preview {name}").into()
        } else {
            format!("Preview {name}, unavailable").into()
        };
        let preview = div()
            .semantic(format!("{id}.preview"), Role::Button)
            .aria_label(preview_name)
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
        let (opening, facts) = (
            (self.key.clone(), self.reference.clone()),
            self.facts.clone(),
        );
        let preview = pressable(
            preview,
            available.then(|| {
                std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                    viewer::open(&opening.0, &opening.1, &facts, window, cx);
                }) as Handler
            }),
        );
        let (key, reference, file) = (
            self.key.clone(),
            self.reference.clone(),
            self.facts.name.clone(),
        );
        let path = self.facts.path.clone();
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
                        .on_press(move |_, cx| download(&key, &reference, &file, cx)),
                    )
                    .when(local, |row| {
                        row.child(
                            IconButton::new(
                                format!("{id}.folder"),
                                "folder",
                                format!("Show {name} in its folder"),
                            )
                            .on_press(move |_, cx| {
                                let path = path.clone();
                                // Explorer starts off the UI thread.
                                cx.background_spawn(async move {
                                    let _ = desktop_host::links::show_in_folder(&path);
                                })
                                .detach();
                            }),
                        )
                    }),
            )
            .when_some(status, |card, status| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::files::source::testing::{asked, png, reference, serving};

    /// M4 re-review R8: a picture's header is checked once its first chunk is in. A 20-megapixel photo is refused there,
    /// as it was once all of it had been read; the megabytes after its header aren't asked for.
    #[gpui::test]
    fn a_picture_too_large_is_refused_from_its_first_chunk(cx: &mut gpui::TestAppContext) {
        let (rpc, calls) = serving(cx, png(5472, 3648, 4 * 1024 * 1024), "image/png");
        let read = futures::executor::block_on(read_image(
            reference(),
            "image/png".into(),
            rpc,
            Signal::new(),
        ));
        assert_eq!(
            read.err(),
            Some(ThumbnailError::Limit("Image exceeds thumbnail dimensions."))
        );
        assert_eq!(
            asked(&calls, "attachments.read"),
            1,
            "the rest of the file was read"
        );
        assert_eq!(
            asked(&calls, "attachments.close"),
            1,
            "its transfer stayed open"
        );
    }

    /// M4 re-review R10: a picture replaced under its key is handed back for GPUI's cache, as one pushed out is.
    #[test]
    fn a_replaced_or_forgotten_picture_is_handed_back() {
        let mut files = Files::default();
        let picture = || Arc::new(Image::from_bytes(ImageFormat::Bmp, Vec::new()));
        let key = |index| FileKey {
            message_id: "m1".into(),
            index,
        };
        let first = picture();
        assert!(files.put(key(0), Thumb::Ready(first.clone())).is_empty());
        let replaced = files.put(key(0), Thumb::Ready(picture()));
        assert!(matches!(replaced.as_slice(), [image] if Arc::ptr_eq(image, &first)));
        // Full, the picture the oldest: the next one pushes it out.
        for index in 1..THUMBNAILS as u64 {
            assert!(files.put(key(index), Thumb::Failed).is_empty());
        }
        assert_eq!(files.put(key(99), Thumb::Refused).len(), 1);
    }
}
