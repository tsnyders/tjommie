//! Files and previews (Phase 3 plan T4.4): Markdown, file cards and their thumbnails, the viewer (images, Markdown
//! files, PDF pages, audio and video), task diffs, and saving a file on this PC.

pub mod card;
pub mod markdown;
pub mod media;
pub mod native;
pub mod pdf;
pub mod source;
pub mod viewer;

/// How a file previews, by its content type (`previewKind` in `file-viewer.tsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Markdown,
    Image,
    Pdf,
    Audio,
    Video,
    Unsupported,
}

pub fn preview_kind(content_type: &str) -> PreviewKind {
    match content_type {
        "text/markdown" => PreviewKind::Markdown,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" => PreviewKind::Image,
        "application/pdf" => PreviewKind::Pdf,
        "audio/wav" | "audio/mpeg" | "audio/mp4" | "audio/ogg" => PreviewKind::Audio,
        "video/mp4" | "video/webm" => PreviewKind::Video,
        _ => PreviewKind::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::{PreviewKind, preview_kind};

    #[test]
    fn each_type_previews_as_the_typescript_said() {
        for (content_type, kind) in [
            ("text/markdown", PreviewKind::Markdown),
            ("image/png", PreviewKind::Image),
            ("image/webp", PreviewKind::Image),
            ("image/svg+xml", PreviewKind::Unsupported),
            ("application/pdf", PreviewKind::Pdf),
            ("audio/ogg", PreviewKind::Audio),
            ("audio/flac", PreviewKind::Unsupported),
            ("video/webm", PreviewKind::Video),
            ("video/quicktime", PreviewKind::Unsupported),
            ("text/plain", PreviewKind::Unsupported),
        ] {
            assert_eq!(preview_kind(content_type), kind, "{content_type}");
        }
    }
}
