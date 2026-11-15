//! The tools that read the page or change how it is shown: console and network logs, a pending file chooser, waiting,
//! screenshots, and media emulation (`nativeBrowserTools` in `native-browser-automation.ts`).

use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Serialize, Serializer};

use super::args::{
    ColorScheme, ConsoleArgs, ConsoleLevel, Contrast, EmulateMediaArgs, FileUploadArgs,
    ForcedColors, ImageType, Media, NetworkRequestArgs, NetworkRequestsArgs, ReducedMotion,
    RequestPart, Scale, ScreenshotArgs, WaitForArgs,
};
use super::protocol::{
    CaptureScreenshotParams, Clip, DomSetFileInputFiles, EmulationSetEmulatedMedia,
    GetResponseBodyParams, MediaFeature, NetworkGetResponseBody, PageCaptureScreenshot,
    PageGetLayoutMetrics, Scalar, SetEmulatedMediaParams, SetFileInputFilesParams,
};
use super::reply::{
    IMAGE_LIMIT, ImageMime, TEXT_LIMIT, ToolContent, ToolError, ToolReply, bounded, simple_regex,
};
use super::run::{Run, target_of};
use super::session::ACTION;
use crate::browser::cdp::NoParams;

/// How a console entry's level ranks against the level a call asks for: errors first, debug last.
fn rank(level: &str) -> u8 {
    match level {
        "error" | "assert" => 0,
        "warning" | "warn" => 1,
        "debug" => 3,
        _ => 2,
    }
}

fn asked(level: ConsoleLevel) -> u8 {
    match level {
        ConsoleLevel::Error => 0,
        ConsoleLevel::Warning => 1,
        ConsoleLevel::Info => 2,
        ConsoleLevel::Debug => 3,
    }
}

/// Headers as a JSON object, in the order the browser sent them.
struct Pairs<'a>(&'a [(String, String)]);

impl Serialize for Pairs<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(name, value)| (name, value)))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Detail<'a> {
    url: &'a str,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u32>,
    request_headers: Pairs<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_headers: Option<Pairs<'a>>,
}

#[derive(Serialize)]
struct BinaryBody {
    base64: String,
    truncated: bool,
}

/// JSON the way `JSON.stringify(value, null, 2)` writes it.
fn pretty<T: Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value).map_err(|error| ToolError(error.to_string()))
}

fn media_text(scheme: ColorScheme) -> &'static str {
    match scheme {
        ColorScheme::Light => "light",
        ColorScheme::Dark => "dark",
        ColorScheme::NoPreference => "no-preference",
    }
}

fn motion_text(motion: ReducedMotion) -> &'static str {
    match motion {
        ReducedMotion::Reduce => "reduce",
        ReducedMotion::NoPreference => "no-preference",
    }
}

fn forced_text(forced: ForcedColors) -> &'static str {
    match forced {
        ForcedColors::Active => "active",
        ForcedColors::Off => "none",
    }
}

fn contrast_text(contrast: Contrast) -> &'static str {
    match contrast {
        Contrast::More => "more",
        Contrast::NoPreference => "no-preference",
    }
}

/// Absent leaves a media feature as it is, `null` resets it, a value sets it. A feature keeps its place in the list when
/// it is set again.
fn change(features: &mut Vec<(String, String)>, name: &str, value: Option<Option<&'static str>>) {
    match value {
        None => {}
        Some(None) => features.retain(|(known, _)| known != name),
        Some(Some(value)) => match features.iter_mut().find(|(known, _)| known == name) {
            Some(entry) => entry.1 = value.to_string(),
            None => features.push((name.to_string(), value.to_string())),
        },
    }
}

impl Run<'_> {
    pub fn console_tool(&self, args: &ConsoleArgs) -> Result<ToolReply, ToolError> {
        self.refresh()?;
        let all = args.all == Some(true);
        let lines: Vec<String> = {
            let state = self.page.state.borrow();
            state
                .console
                .iter()
                .filter(|entry| {
                    (all || entry.document == state.document)
                        && rank(&entry.level) <= asked(args.level)
                })
                .map(|entry| format!("[{}] {}", entry.level, entry.text))
                .collect()
        };
        let text = if lines.is_empty() {
            "No matching observed console messages.".to_string()
        } else {
            lines.join("\n")
        };
        self.result(&text, args.filename.as_deref())
    }

    pub fn requests_tool(&self, args: &NetworkRequestsArgs) -> Result<ToolReply, ToolError> {
        let pattern = args.filter.as_deref().map(simple_regex).transpose()?;
        self.refresh()?;
        let everything = args.static_resources || args.include_static == Some(true);
        let lines: Vec<String> = self
            .page
            .state
            .borrow()
            .requests
            .iter()
            .filter(|entry| {
                (everything
                    || ["Fetch", "XHR", "Document"].contains(&entry.kind.as_str())
                    || entry.status.is_some_and(|status| status >= 400))
                    && pattern
                        .as_ref()
                        .is_none_or(|pattern| pattern.is_match(&entry.url))
            })
            .map(|entry| {
                let status = entry
                    .status
                    .map_or_else(|| "pending".to_string(), |status| status.to_string());
                format!(
                    "{}. {} {} => {}",
                    entry.index, entry.method, entry.url, status
                )
            })
            .collect();
        let text = if lines.is_empty() {
            "No matching observed requests.".to_string()
        } else {
            lines.join("\n")
        };
        self.result(&text, args.filename.as_deref())
    }

    pub async fn request_tool(&self, args: &NetworkRequestArgs) -> Result<ToolReply, ToolError> {
        self.refresh()?;
        let entry = self
            .page
            .state
            .borrow()
            .requests
            .iter()
            .find(|request| u64::from(request.index) == args.index)
            .cloned()
            .ok_or_else(|| {
                ToolError::new("That request is no longer in the bounded network log.")
            })?;
        let text = match args.part {
            Some(RequestPart::RequestBody) => "Request bodies are not retained.".to_string(),
            Some(RequestPart::RequestHeaders) => pretty(&Pairs(&entry.request_headers))?,
            Some(RequestPart::ResponseHeaders) => pretty(&Pairs(
                entry.response_headers.as_deref().unwrap_or_default(),
            ))?,
            Some(RequestPart::ResponseBody) => {
                let body = self
                    .cdp::<NetworkGetResponseBody>(&GetResponseBodyParams {
                        request_id: entry.id.clone(),
                    })
                    .await?;
                if body.base64_encoded {
                    pretty(&BinaryBody {
                        truncated: body.body.chars().count() > TEXT_LIMIT,
                        base64: bounded(&body.body, TEXT_LIMIT),
                    })?
                } else {
                    bounded(&body.body, TEXT_LIMIT)
                }
            }
            None => pretty(&Detail {
                url: &entry.url,
                method: &entry.method,
                status: entry.status,
                request_headers: Pairs(&entry.request_headers),
                response_headers: entry.response_headers.as_deref().map(Pairs),
            })?,
        };
        self.result(&text, args.filename.as_deref())
    }

    pub async fn upload_tool(&self, args: &FileUploadArgs) -> Result<ToolReply, ToolError> {
        self.refresh()?;
        let input = self.page.state.borrow().file_input.ok_or_else(|| {
            ToolError::new("No bot-opened file chooser is pending. Click the file input first.")
        })?;
        if let Some(paths) = &args.paths {
            let files = self.uploads(paths)?;
            self.cdp::<DomSetFileInputFiles>(&SetFileInputFilesParams {
                backend_node_id: input,
                files,
            })
            .await?;
        }
        self.page.state.borrow_mut().file_input = None;
        self.shown().await
    }

    pub async fn wait_tool(&self, args: &WaitForArgs) -> Result<ToolReply, ToolError> {
        if args.time.is_none() && args.text.is_none() && args.text_gone.is_none() {
            return Err(ToolError::new("Supply time, text or textGone."));
        }
        if let Some(seconds) = args.time.filter(|seconds| *seconds > 0.0) {
            self.ctx.delay((seconds * 1000.0) as u64).await?;
        }
        let present = args.text.as_deref().map_or_else(
            || "true".to_string(),
            |text| {
                format!(
                    "body.includes({})",
                    serde_json::to_string(text).unwrap_or_default()
                )
            },
        );
        let gone = args.text_gone.as_deref().map_or_else(
            || "true".to_string(),
            |text| {
                format!(
                    "!body.includes({})",
                    serde_json::to_string(text).unwrap_or_default()
                )
            },
        );
        let expression = format!(
            "(() => {{ const body = document.body?.innerText ?? ''; return {present} && {gone}; }})()"
        );
        let deadline = Instant::now() + ACTION;
        loop {
            if self.eval::<Scalar>(&expression).await? == Some(Scalar::Bool(true)) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(ToolError::new(
                    "The requested text condition did not become true.",
                ));
            }
            self.ctx.delay(100).await?;
        }
        self.shown().await
    }

    pub async fn screenshot_tool(&self, args: &ScreenshotArgs) -> Result<ToolReply, ToolError> {
        let kind = args.image_type.unwrap_or(ImageType::Png);
        let named = args.target.is_some() || args.reference.is_some();
        let full_page = args.full_page == Some(true);
        if full_page && named {
            return Err(ToolError::new(
                "Choose fullPage or an element target, not both.",
            ));
        }
        let ratio = match self.eval::<Scalar>("window.devicePixelRatio || 1").await? {
            Some(Scalar::Number(ratio)) if ratio > 0.0 => ratio,
            _ => 1.0,
        };
        let scale = if args.scale == Some(Scale::Device) {
            1.0
        } else {
            1.0 / ratio
        };
        let clip = if named {
            let item = self
                .actionable(
                    target_of(args.target.as_deref(), args.reference.as_deref())?,
                    false,
                )
                .await?;
            let rect = self.rect(&item.object_id).await?;
            // The element is in view now: its box is relative to where the page is scrolled to now.
            let view = self
                .cdp::<PageGetLayoutMetrics>(&NoParams {})
                .await?
                .css_visual_viewport;
            Clip {
                x: rect.x + view.page_x,
                y: rect.y + view.page_y,
                width: rect.width,
                height: rect.height,
                scale,
            }
        } else {
            let metrics = self.cdp::<PageGetLayoutMetrics>(&NoParams {}).await?;
            if full_page {
                Clip {
                    x: 0.0,
                    y: 0.0,
                    width: metrics.css_content_size.width,
                    height: metrics.css_content_size.height,
                    scale,
                }
            } else {
                Clip {
                    x: metrics.css_visual_viewport.page_x,
                    y: metrics.css_visual_viewport.page_y,
                    width: metrics.css_visual_viewport.client_width,
                    height: metrics.css_visual_viewport.client_height,
                    scale,
                }
            }
        };
        let finite = [clip.x, clip.y, clip.width, clip.height, clip.scale]
            .iter()
            .all(|value| value.is_finite());
        if !finite
            || clip.width <= 0.0
            || clip.height <= 0.0
            || clip.width * clip.height > 16_000_000.0
            || clip.width > 16000.0
            || clip.height > 16000.0
            || clip.scale <= 0.0
            || clip.scale > 4.0
        {
            return Err(ToolError::new(
                "The screenshot exceeds the 16-megapixel limit. Use a smaller viewport or an element target.",
            ));
        }
        let (format, mime) = match kind {
            ImageType::Png => ("png", ImageMime::Png),
            ImageType::Jpeg => ("jpeg", ImageMime::Jpeg),
            ImageType::Webp => ("webp", ImageMime::Webp),
        };
        let reply = self
            .cdp::<PageCaptureScreenshot>(&CaptureScreenshotParams {
                format: format.to_string(),
                quality: (kind != ImageType::Png).then_some(75),
                clip: Some(clip),
                capture_beyond_viewport: full_page || named,
            })
            .await?;
        let too_big = || {
            ToolError::new(
                "The screenshot exceeds the 2 MiB image limit. Use JPEG or a smaller target.",
            )
        };
        if reply.data.is_empty() || reply.data.len() > IMAGE_LIMIT.div_ceil(3) * 4 {
            return Err(too_big());
        }
        let bytes = STANDARD.decode(&reply.data).map_err(|_| too_big())?;
        if bytes.len() > IMAGE_LIMIT {
            return Err(ToolError::new(
                "The screenshot exceeds the 2 MiB image limit.",
            ));
        }
        let saved = match &args.filename {
            Some(name) => format!("Saved screenshot: {}", self.save(&bytes, name)?.display()),
            None => "Screenshot of the shared native browser tab.".to_string(),
        };
        Ok(ToolReply {
            content: vec![
                ToolContent::Text { text: saved },
                ToolContent::Image {
                    data: reply.data,
                    mime_type: mime,
                },
            ],
            is_error: false,
        })
    }

    pub async fn emulate_media_tool(
        &self,
        args: &EmulateMediaArgs,
    ) -> Result<ToolReply, ToolError> {
        let (media, media_type) = {
            let mut state = self.page.state.borrow_mut();
            change(
                &mut state.media,
                "prefers-color-scheme",
                args.color_scheme.map(|value| value.map(media_text)),
            );
            change(
                &mut state.media,
                "prefers-reduced-motion",
                args.reduced_motion.map(|value| value.map(motion_text)),
            );
            change(
                &mut state.media,
                "forced-colors",
                args.forced_colors.map(|value| value.map(forced_text)),
            );
            change(
                &mut state.media,
                "prefers-contrast",
                args.contrast.map(|value| value.map(contrast_text)),
            );
            if let Some(media) = args.media {
                state.media_type = media.map(|media| match media {
                    Media::Screen => "screen".to_string(),
                    Media::Print => "print".to_string(),
                });
            }
            (state.media.clone(), state.media_type.clone())
        };
        self.cdp::<EmulationSetEmulatedMedia>(&SetEmulatedMediaParams {
            media: media_type.unwrap_or_default(),
            features: media
                .into_iter()
                .map(|(name, value)| MediaFeature { name, value })
                .collect(),
        })
        .await?;
        self.shown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_features_are_set_reset_and_left_alone_in_place() {
        let mut features = Vec::new();
        change(&mut features, "prefers-color-scheme", Some(Some("dark")));
        change(
            &mut features,
            "prefers-reduced-motion",
            Some(Some("reduce")),
        );
        change(&mut features, "prefers-color-scheme", Some(Some("light")));
        assert_eq!(
            features,
            vec![
                ("prefers-color-scheme".to_string(), "light".to_string()),
                ("prefers-reduced-motion".to_string(), "reduce".to_string())
            ],
            "a feature set again keeps its place"
        );
        change(&mut features, "forced-colors", None);
        assert_eq!(features.len(), 2, "absent leaves things as they are");
        change(&mut features, "prefers-color-scheme", Some(None));
        assert_eq!(
            features,
            vec![("prefers-reduced-motion".to_string(), "reduce".to_string())]
        );
    }

    #[test]
    fn console_levels_rank_errors_first() {
        assert_eq!(
            [
                "error", "assert", "warning", "warn", "log", "info", "debug", "table"
            ]
            .map(rank),
            [0, 0, 1, 1, 2, 2, 3, 2]
        );
        assert_eq!(
            [
                ConsoleLevel::Error,
                ConsoleLevel::Warning,
                ConsoleLevel::Info,
                ConsoleLevel::Debug
            ]
            .map(asked),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn a_request_is_written_as_pretty_json_with_headers_in_order() {
        let request = vec![
            ("Accept".to_string(), "text/html".to_string()),
            ("Authorization".to_string(), "[redacted]".to_string()),
        ];
        let text = pretty(&Detail {
            url: "http://x/",
            method: "GET",
            status: Some(200),
            request_headers: Pairs(&request),
            response_headers: None,
        })
        .unwrap();
        assert_eq!(
            text,
            "{\n  \"url\": \"http://x/\",\n  \"method\": \"GET\",\n  \"status\": 200,\n  \"requestHeaders\": {\n    \"Accept\": \"text/html\",\n    \"Authorization\": \"[redacted]\"\n  }\n}"
        );
        let pending = pretty(&Detail {
            url: "u",
            method: "GET",
            status: None,
            request_headers: Pairs(&[]),
            response_headers: Some(Pairs(&[])),
        })
        .unwrap();
        assert!(!pending.contains("status") && pending.contains("\"responseHeaders\": {}"));
    }
}
