//! A PDF in the viewer (`pdf-file-viewer.tsx`): a page at a time, with Previous and Next and zoom from 0.5× to 4×.
//! pdfium (`desktop_host::pdf`) opens and draws it off the UI thread, on the live runtime's blocking pool, reading the
//! file through the transfer in 256 KiB blocks, so a 200 MiB PDF is never read whole. A drawn page reaches GPUI as a
//! BMP; the page it replaces is forgotten.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use desktop_host::pdf::{PDFIUM_SHA256, PdfDocument, PdfError, RangeSource, load_library};
use gpui::{
    AnyElement, Context, Image, ImageFormat, IntoElement, ParentElement as _, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, img, px,
};

use super::source::Opened;
use crate::guard::guarded_async;
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::IconButton;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 4.;
/// The most pixels a page is drawn with, as the TypeScript's canvas.
const PAGE_PIXELS: f32 = 8. * 1024. * 1024.;
const UNREADABLE: &str = "This PDF is corrupt, unsupported, or its local reader is unavailable. Download is still available.";
const UNDRAWN: &str = "The PDF page could not be rendered. Download is still available.";
// ponytail: no password prompt yet (the TypeScript had one); a locked PDF says so and offers Download.
const LOCKED: &str = "This PDF is protected by a password. Download is still available.";

/// The transfer as pdfium reads it: each read waits for its block, on the drawing thread.
struct Transfer(Arc<Opened>);

impl RangeSource for Transfer {
    fn len(&self) -> u64 {
        self.0.info().size
    }

    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let end = offset.saturating_add(len as u64);
        futures::executor::block_on(self.0.read_span(offset, end))
    }
}

/// pdfium.dll: the installed app's `resources\pdfium.dll`; else `TJOMMIE_PDFIUM_DLL`; else, for a run from source, the
/// one `tools\fetch-pdfium.ps1` fetched (`tjommie-rs\resources\pdfium\`, two folders up from `target\debug`). Only the
/// pinned build loads, wherever it is found.
fn pdfium_dll() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let folder = exe.parent()?;
    let bundled = folder.join("resources").join("pdfium.dll");
    if bundled.is_file() {
        return Some(bundled);
    }
    if let Some(named) = std::env::var_os("TJOMMIE_PDFIUM_DLL").filter(|named| !named.is_empty()) {
        return Some(named.into());
    }
    Some(
        folder
            .parent()?
            .parent()?
            .join("resources")
            .join("pdfium")
            .join("pdfium.dll"),
    )
}

/// Opens the PDF with the pinned pdfium (loaded once per process).
fn open_document(source: Transfer) -> Result<PdfDocument, PdfError> {
    let dll = pdfium_dll().ok_or(PdfError::LibraryMissing)?;
    load_library(&dll, PDFIUM_SHA256)?.open(source)
}

/// Runs pdfium's work on the live runtime's blocking pool; with no runtime (view tests) it runs where it is.
async fn off_thread<T: Send + 'static>(
    tokio: Option<tokio::runtime::Handle>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    match tokio {
        Some(handle) => handle.spawn_blocking(work).await.ok(),
        None => Some(work()),
    }
}

struct Page {
    index: u32,
    zoom: f32,
    image: Arc<Image>,
    /// Its size on screen, in GPUI's pixels.
    width: f32,
    height: f32,
}

pub struct PdfView {
    id: SharedString,
    tokio: Option<tokio::runtime::Handle>,
    document: Option<Arc<Mutex<PdfDocument>>>,
    pages: u32,
    page: u32,
    zoom: f32,
    scale: f32,
    shown: Option<Page>,
    /// The page and zoom being drawn now.
    drawing: Option<(u32, f32)>,
    problem: Option<SharedString>,
}

impl PdfView {
    pub fn new(id: SharedString, opened: Arc<Opened>, cx: &mut Context<Self>) -> Self {
        let tokio = cx
            .try_global::<crate::live::Tokio>()
            .map(|tokio| tokio.0.clone());
        let opening = tokio.clone();
        cx.spawn(async move |this, cx| {
            guarded_async("open a PDF", async move {
                let document = off_thread(opening, move || open_document(Transfer(opened))).await;
                let _ = this.update(cx, |this, cx| {
                    match document {
                        Some(Ok(document)) => {
                            this.pages = document.page_count();
                            this.document = Some(Arc::new(Mutex::new(document)));
                        }
                        Some(Err(PdfError::Locked)) => this.problem = Some(LOCKED.into()),
                        Some(Err(error)) => {
                            log::warn!("A PDF preview didn't open: {error}");
                            this.problem = Some(UNREADABLE.into());
                        }
                        None => this.problem = Some(UNREADABLE.into()),
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
        PdfView {
            id,
            tokio,
            document: None,
            pages: 0,
            page: 0,
            zoom: 1.,
            scale: 1.,
            shown: None,
            drawing: None,
            problem: None,
        }
    }

    fn go(&mut self, page: u32, cx: &mut Context<Self>) {
        if page < self.pages {
            self.page = page;
            cx.notify();
        }
    }

    fn zoom(&mut self, by: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom * by).clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    /// Draws the page and zoom asked for, unless it's on screen or on its way. A page asked for while another is
    /// drawn waits for it, then the newest wish is drawn.
    fn draw(&mut self, cx: &mut Context<Self>) {
        let wanted = (self.page, self.zoom);
        let Some(document) = self.document.clone() else {
            return;
        };
        if self.drawing.is_some()
            || self
                .shown
                .as_ref()
                .is_some_and(|page| (page.index, page.zoom) == wanted)
        {
            return;
        }
        self.drawing = Some(wanted);
        let (tokio, scale) = (self.tokio.clone(), self.scale);
        cx.spawn(async move |this, cx| {
            guarded_async("draw a PDF page", async move {
                let drawn = off_thread(tokio, move || {
                    let document = document
                        .lock()
                        .map_err(|_| "the document is gone".to_owned())?;
                    let (width_pt, height_pt) = document
                        .page_size(wanted.0)
                        .map_err(|error| error.to_string())?;
                    // The page at its own size times the zoom, in device pixels, within the pixel budget.
                    let (width, height) = (width_pt * wanted.1, height_pt * wanted.1);
                    let ratio = scale.min((PAGE_PIXELS / (width * height).max(1.)).sqrt());
                    let target = (width * ratio).floor().max(1.) as u32;
                    let page = document
                        .render_page(wanted.0, target)
                        .map_err(|error| error.to_string())?;
                    let bmp = bmp(page.width, page.height, &page.bgra)
                        .ok_or_else(|| "the page's pixels don't fit its size".to_owned())?;
                    Ok::<_, String>((bmp, width, height))
                })
                .await;
                let _ = this.update(cx, |this, cx| {
                    this.drawing = None;
                    match drawn {
                        Some(Ok((bmp, width, height))) => {
                            let page = Page {
                                index: wanted.0,
                                zoom: wanted.1,
                                image: Arc::new(Image::from_bytes(ImageFormat::Bmp, bmp)),
                                width,
                                height,
                            };
                            if let Some(old) = this.shown.replace(page) {
                                old.image.remove_asset(cx);
                            }
                        }
                        Some(Err(error)) => {
                            log::warn!("A PDF page wasn't drawn: {error}");
                            this.problem = Some(UNDRAWN.into());
                        }
                        None => this.problem = Some(UNDRAWN.into()),
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
    }
}

impl Drop for PdfView {
    fn drop(&mut self) {
        // The document goes with the last draw that holds it; the page's picture with the app's next frame.
        self.shown.take();
    }
}

/// A page's BGRA pixels (`width` × 4 bytes a row) as a 24-bit top-down BMP, which GPUI decodes. PDF pages are drawn
/// on white, so nothing is lost with the alpha.
pub fn bmp(width: u32, height: u32, bgra: &[u8]) -> Option<Vec<u8>> {
    let (columns, rows) = (width as usize, height as usize);
    let stride = (columns * 3 + 3) & !3;
    let pixels = stride.checked_mul(rows)?;
    let size = pixels.checked_add(54)?;
    if columns == 0 || rows == 0 || bgra.len() < columns.checked_mul(rows)?.checked_mul(4)? {
        return None;
    }
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&u32::try_from(size).ok()?.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&i32::try_from(width).ok()?.to_le_bytes());
    // A negative height: the rows run top to bottom.
    out.extend_from_slice(&(-i32::try_from(height).ok()?).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(pixels).ok()?.to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for line in bgra.chunks_exact(columns * 4).take(rows) {
        for pixel in line.chunks_exact(4) {
            out.extend_from_slice(pixel.get(..3)?);
        }
        out.resize(out.len() + stride - columns * 3, 0);
    }
    (out.len() == size).then_some(out)
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale = window.scale_factor();
        self.draw(cx);
        let theme = theme(cx);
        let colors = palette(cx);
        let id = self.id.clone();
        if let Some(problem) = &self.problem {
            return div()
                .semantic(format!("{id}.problem"), Role::Alert)
                .aria_label(problem.clone())
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.danger.text))
                .child(problem.clone())
                .into_any_element();
        }
        let entity = cx.entity().downgrade();
        let (back, next, out, inn) = (entity.clone(), entity.clone(), entity.clone(), entity);
        let (page, pages, zoom) = (self.page, self.pages, self.zoom);
        let loaded = self.document.is_some();
        let status = if loaded {
            format!("{} / {pages}", page + 1)
        } else {
            "Loading PDF…".to_owned()
        };
        let picture: Option<AnyElement> = self.shown.as_ref().map(|shown| {
            div()
                .w(px(shown.width))
                .h(px(shown.height))
                .flex_none()
                .bg(gpui::white())
                .child(img(shown.image.clone()).size_full())
                .into_any_element()
        });
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
                        IconButton::new(format!("{id}.previous"), "chevron-left", "Previous page")
                            .disabled(!loaded || page == 0)
                            .on_press(move |_, cx| {
                                let _ =
                                    back.update(cx, |this, cx| this.go(page.saturating_sub(1), cx));
                            }),
                    )
                    .child(
                        div()
                            .semantic(format!("{id}.page"), Role::Status)
                            .aria_label(status.clone())
                            .type_style(&theme.typography.desktop.meta)
                            .text_color(hsla(colors.text.secondary))
                            .child(status),
                    )
                    .child(
                        IconButton::new(format!("{id}.next"), "chevron-right", "Next page")
                            .disabled(!loaded || page + 1 >= pages)
                            .on_press(move |_, cx| {
                                let _ = next.update(cx, |this, cx| this.go(page + 1, cx));
                            }),
                    )
                    .child(
                        IconButton::new(format!("{id}.zoom-out"), "minus", "Zoom PDF out")
                            .disabled(zoom <= ZOOM_MIN)
                            .on_press(move |_, cx| {
                                let _ = out.update(cx, |this, cx| this.zoom(1. / 1.25, cx));
                            }),
                    )
                    .child(
                        IconButton::new(format!("{id}.zoom-in"), "plus", "Zoom PDF in")
                            .disabled(zoom >= ZOOM_MAX)
                            .on_press(move |_, cx| {
                                let _ = inn.update(cx, |this, cx| this.zoom(1.25, cx));
                            }),
                    ),
            )
            .child(
                div()
                    .semantic(format!("{id}.sheet"), Role::Image)
                    .aria_label(format!("PDF page {}", page + 1))
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .children(picture),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::bmp;

    #[test]
    fn a_page_becomes_a_top_down_bmp_with_padded_rows() {
        // 2 × 2: blue, green / red, white.
        let bgra = [
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let out = bmp(2, 2, &bgra).unwrap();
        assert_eq!(
            out.len(),
            54 + 2 * 8,
            "two rows of 6 bytes, each padded to 8"
        );
        assert_eq!(&out[..2], b"BM");
        assert_eq!(
            i32::from_le_bytes(out[22..26].try_into().unwrap()),
            -2,
            "top-down"
        );
        assert_eq!(u16::from_le_bytes(out[28..30].try_into().unwrap()), 24);
        assert_eq!(&out[54..62], &[255, 0, 0, 0, 255, 0, 0, 0]);
        assert_eq!(&out[62..70], &[0, 0, 255, 255, 255, 255, 0, 0]);
        assert!(bmp(2, 2, &bgra[..12]).is_none(), "too few pixels");
        assert!(bmp(0, 2, &bgra).is_none());
    }
}
