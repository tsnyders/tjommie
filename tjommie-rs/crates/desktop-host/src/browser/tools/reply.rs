//! What a tool answers: text and images, bounded, as the daemon's MCP proxy expects them
//! (`{"content": [...], "isError": true}`), and the text helpers every tool shares.

use serde::Serialize;

/// The longest text a tool returns, in characters.
pub const TEXT_LIMIT: usize = 128 * 1024;
/// The largest image a tool returns, in bytes.
pub const IMAGE_LIMIT: usize = 2 * 1024 * 1024;

/// `value` cut to `limit` characters, with a marker if anything was cut. (TypeScript counted UTF-16 units; characters
/// keep a cut from landing inside one.)
pub fn bounded(value: &str, limit: usize) -> String {
    if value.chars().count() > limit {
        let kept: String = value.chars().take(limit).collect();
        format!("{kept}\n[truncated]")
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ImageMime {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum ToolContent {
    Text { text: String },
    Image { data: String, mime_type: ImageMime },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolReply {
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError", skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

impl ToolReply {
    /// A text answer, bounded to [`TEXT_LIMIT`].
    pub fn text(value: &str) -> Self {
        Self {
            content: vec![ToolContent::Text {
                text: bounded(value, TEXT_LIMIT),
            }],
            is_error: false,
        }
    }

    /// A failed tool: the message is what the model reads.
    pub fn error(message: &str) -> Self {
        Self {
            is_error: true,
            ..Self::text(message)
        }
    }

    /// The text of the answer, joined (what the model reads).
    pub fn words(&self) -> String {
        self.content
            .iter()
            .filter_map(|item| match item {
                ToolContent::Text { text } => Some(text.as_str()),
                ToolContent::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Why a tool stopped, in the words the model reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(message: &str) -> Self {
        Self(message.to_string())
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ToolError {}

impl From<crate::browser::engine::EngineError> for ToolError {
    fn from(error: crate::browser::engine::EngineError) -> Self {
        Self(error.to_string())
    }
}

impl From<crate::browser::cdp::CdpError> for ToolError {
    fn from(error: crate::browser::cdp::CdpError) -> Self {
        Self(error.to_string())
    }
}

impl From<crate::browser::com::ComError> for ToolError {
    fn from(error: crate::browser::com::ComError) -> Self {
        Self(error.to_string())
    }
}

impl From<crate::browser::policy::PolicyError> for ToolError {
    fn from(error: crate::browser::policy::PolicyError) -> Self {
        Self(error.to_string())
    }
}

/// A search pattern the tools accept (`browser_find`, `browser_network_requests`): plain text, or `/pattern/flags` with
/// flags `i` and `m`, at most 128 characters, with no groups, no counted repetition, no back-references and at most one
/// `*`, `+` or `?`: matching stays bounded on text a page controls.
pub fn simple_regex(value: &str) -> Result<regex::Regex, ToolError> {
    let (pattern, flags) = match value
        .strip_prefix('/')
        .and_then(|rest| rest.rfind('/').map(|at| rest.split_at(at)))
    {
        Some((pattern, tail))
            if tail
                .get(1..)
                .is_some_and(|f| f.chars().all(|c| c == 'i' || c == 'm')) =>
        {
            (pattern, tail.get(1..).unwrap_or_default())
        }
        _ => (value, ""),
    };
    let mut repeats = 0;
    let mut escaped = false;
    let mut digit_escape = false;
    for c in pattern.chars() {
        if escaped {
            escaped = false;
            digit_escape |= ('1'..='9').contains(&c);
            continue;
        }
        match c {
            '\\' => escaped = true,
            '*' | '+' | '?' => repeats += 1,
            _ => {}
        }
    }
    if pattern.chars().count() > 128
        || pattern.contains(['(', ')', '{', '}'])
        || digit_escape
        || repeats > 1
    {
        return Err(ToolError::new(
            "Use a simple regular expression without groups or multiple repetitions, or use plain text.",
        ));
    }
    regex::RegexBuilder::new(pattern)
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .size_limit(1 << 20)
        .build()
        .map_err(|_| ToolError::new("Invalid regular expression."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_with_a_marker_and_replies_serialize_as_the_daemon_reads_them() {
        assert_eq!(bounded("abc", 3), "abc");
        assert_eq!(bounded("abcd", 3), "abc\n[truncated]");
        assert_eq!(bounded("ééé", 2), "éé\n[truncated]");
        let text = serde_json::to_string(&ToolReply::text("hi")).unwrap();
        assert_eq!(text, r#"{"content":[{"type":"text","text":"hi"}]}"#);
        let error = serde_json::to_string(&ToolReply::error("no")).unwrap();
        assert_eq!(
            error,
            r#"{"content":[{"type":"text","text":"no"}],"isError":true}"#
        );
        let image = ToolReply {
            content: vec![
                ToolContent::Text { text: "t".into() },
                ToolContent::Image {
                    data: "QQ==".into(),
                    mime_type: ImageMime::Jpeg,
                },
            ],
            is_error: false,
        };
        assert_eq!(
            serde_json::to_string(&image).unwrap(),
            r#"{"content":[{"type":"text","text":"t"},{"type":"image","data":"QQ==","mimeType":"image/jpeg"}]}"#
        );
        assert_eq!(image.words(), "t");
    }

    #[test]
    fn a_search_pattern_is_plain_text_or_a_bounded_regex() {
        assert!(
            simple_regex("save")
                .unwrap()
                .is_match("a Save button".to_lowercase().as_str())
        );
        assert!(simple_regex("/SAVE/i").unwrap().is_match("a Save button"));
        assert!(!simple_regex("/SAVE/").unwrap().is_match("a Save button"));
        assert!(simple_regex(r"/a+b/").is_ok());
        for refused in ["(a)", "a{2}", r"\1", "a+b*", "a*b+c?", &"x".repeat(129)] {
            assert!(simple_regex(refused).is_err(), "{refused}");
        }
        // An escaped repetition character is a literal, not a repetition.
        assert!(simple_regex(r"a\+b\*c+").is_ok());
        assert_eq!(
            simple_regex("/[/").unwrap_err().0,
            "Invalid regular expression."
        );
    }
}
