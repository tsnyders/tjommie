//! Markdown (Phase 3 plan T4.4; `markdown.tsx`): bot replies, drawn by gpui-kit's `TextView` (GFM, with text
//! selection and the streaming word fade). A reply is rewritten first, with 0.5.x's rules: raw HTML never renders,
//! an unsafe address is dropped, only web links open (in the default browser), a web image becomes a link to it, and
//! any other image shows its alt text. The rewrite reads the reply with the parser `TextView` uses (markdown-rs, the
//! remark family 0.5.x's react-markdown is from) and splices the source, so everything else reads as written.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, ImageCacheError, ImageSource, SharedString, StyleRefinement, Styled as _, Window, px,
};
use gpui_kit::base::{TextView, TextViewMotion, TextViewStyle};
use markdown::ParseOptions;
use markdown::mdast::Node;

use crate::guard::guarded;
use crate::ui::style::{hsla, palette, r, theme};

/// The desktop app opens web links in the default browser and blocks every other target.
pub fn web(url: &str) -> bool {
    let lower = url.get(..6).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http:") || lower.starts_with("https:")
}

/// Opens `url` in the default browser if it is a web link; any other target does nothing.
pub fn open_web(url: &str, cx: &App) {
    if web(url) {
        cx.open_url(url);
    }
}

/// react-markdown's `defaultUrlTransform`: a relative address, or one whose protocol is http(s), irc(s), mailto or
/// xmpp, stays; any other (`javascript:`, `data:`, `vbscript:`, ...) is dropped as unsafe.
fn safe(url: &str) -> &str {
    let at = |c: char| url.find(c);
    let Some(colon) = at(':') else { return url };
    let before = |other: Option<usize>| other.is_some_and(|index| colon > index);
    let protocol = url.get(..colon).unwrap_or_default();
    let known = ["http", "https", "irc", "ircs", "mailto", "xmpp"]
        .iter()
        .any(|safe| protocol.eq_ignore_ascii_case(safe));
    if before(at('/')) || before(at('?')) || before(at('#')) || known {
        url
    } else {
        ""
    }
}

/// Text that reads as written: every ASCII punctuation mark escaped, so nothing in it becomes Markdown (or an
/// autolink) again.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\r' => out.push(' '),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// A link to a web address, with `text` as its (already Markdown) text.
fn link(text: &str, url: &str) -> String {
    let url: String = url
        .chars()
        .filter(|c| !matches!(c, '\n' | '\r'))
        .flat_map(|c| match c {
            '<' => "%3C".chars().collect::<Vec<_>>(),
            '>' => "%3E".chars().collect(),
            c => vec![c],
        })
        .collect();
    format!("[{text}](<{url}>)")
}

/// The reply as Markdown that is safe to draw. A reply that doesn't parse is shown as its plain text.
pub fn sanitize(text: &str) -> String {
    let Ok(tree) = markdown::to_mdast(text, &ParseOptions::gfm()) else {
        return plain(text);
    };
    let mut definitions = HashMap::new();
    collect_definitions(&tree, &mut definitions);
    let rewrite = Rewrite {
        source: text,
        definitions,
    };
    let mut out = String::with_capacity(text.len());
    rewrite.node(&tree, None, &mut out);
    out
}

fn collect_definitions<'a>(node: &'a Node, into: &mut HashMap<String, &'a str>) {
    if let Node::Definition(definition) = node {
        into.insert(
            definition.identifier.to_lowercase(),
            definition.url.as_str(),
        );
    }
    for child in node.children().into_iter().flatten() {
        collect_definitions(child, into);
    }
}

fn span(node: &Node) -> Option<Range<usize>> {
    node.position()
        .map(|position| position.start.offset..position.end.offset)
}

struct Rewrite<'a> {
    source: &'a str,
    definitions: HashMap<String, &'a str>,
}

impl Rewrite<'_> {
    fn text(&self, range: Range<usize>) -> &str {
        self.source.get(range).unwrap_or_default()
    }

    fn url_of(&self, identifier: &str) -> &str {
        self.definitions
            .get(&identifier.to_lowercase())
            .copied()
            .unwrap_or_default()
    }

    /// Writes `node` as it should read. `inside` is the web address of the link around it, if any.
    fn node(&self, node: &Node, inside: Option<&str>, out: &mut String) {
        match node {
            // Raw HTML never renders, and an address that can't open goes with its links.
            Node::Html(_) => {}
            Node::Definition(definition) if !web(safe(&definition.url)) => {}
            Node::Image(image) => out.push_str(&self.image(&image.alt, &image.url, inside)),
            Node::ImageReference(image) => {
                let url = self.url_of(&image.identifier);
                out.push_str(&self.image(&image.alt, url, inside));
            }
            Node::Link(found) => self.link(node, &found.url, out),
            Node::LinkReference(found) => {
                let url = self.url_of(&found.identifier);
                self.link(node, url, out);
            }
            _ => self.copy(node, inside, out),
        }
    }

    /// The node's own source, with its children rewritten.
    fn copy(&self, node: &Node, inside: Option<&str>, out: &mut String) {
        let Some(whole) = span(node) else { return };
        let children: Vec<&Node> = node
            .children()
            .into_iter()
            .flatten()
            .filter(|child| span(child).is_some())
            .collect();
        let mut at = whole.start;
        for child in children {
            let Some(range) = span(child) else { continue };
            out.push_str(self.text(at..range.start));
            self.node(child, inside, out);
            at = range.end;
        }
        out.push_str(self.text(at..whole.end));
    }

    /// A link that can open keeps its address, its text rewritten; anything else (a target dropped as unsafe, an
    /// email address) shows as its plain text.
    fn link(&self, node: &Node, url: &str, out: &mut String) {
        let url = safe(url);
        if web(url) {
            // Kept as written (an autolink stays bare), so its text alone is rewritten.
            self.copy(node, Some(url), out);
        } else {
            let mut text = String::new();
            for child in node.children().into_iter().flatten() {
                self.node(child, None, &mut text);
            }
            // Its children were written as Markdown; a link that isn't one any more must not come back as one.
            let shown = markdown::to_mdast(&text, &ParseOptions::gfm())
                .map(|tree| tree.to_string())
                .unwrap_or(text);
            out.push_str(&plain(&shown));
        }
    }

    /// Images never load. A web image becomes a link to it; any other image shows its alt text. Inside a link it
    /// shows its alt text, or its address, or else (an address dropped as unsafe) the link's own address, so the link
    /// never ends up with nothing to read or click.
    fn image(&self, alt: &str, url: &str, inside: Option<&str>) -> String {
        let url = safe(url);
        match inside {
            Some(around) => {
                let shown = [alt, url, around]
                    .into_iter()
                    .find(|text| !text.is_empty())
                    .unwrap_or_default();
                plain(shown)
            }
            None if web(url) => link(&plain(if alt.is_empty() { url } else { alt }), url),
            None => plain(alt),
        }
    }
}

/// A source that never loads: chat images become links before they are drawn, so this is the backstop.
fn blocked() -> ImageSource {
    ImageSource::Custom(Arc::new(|_, _| {
        Some(Err(ImageCacheError::Io(Arc::new(std::io::Error::other(
            "images in a reply never load",
        )))))
    }))
}

/// The rewritten text of one reply, kept while it is drawn so a frame doesn't parse it again.
struct Memo {
    source: SharedString,
    safe: SharedString,
}

/// A bot reply's Markdown, as Aegis draws prose: body type, ink links underlined, code on a raised white slip, inline
/// code on stone.
pub fn reply(id: &str, text: &SharedString, window: &mut Window, cx: &mut App) -> TextView {
    let memo = window.use_keyed_state(SharedString::from(format!("{id}.md")), cx, |_, _| Memo {
        source: SharedString::default(),
        safe: SharedString::default(),
    });
    if memo.read(cx).source != *text {
        let safe = SharedString::from(sanitize(text));
        memo.update(cx, |memo, _| {
            memo.source = text.clone();
            memo.safe = safe;
        });
    }
    let safe = memo.read(cx).safe.clone();
    styled(
        TextView::markdown(SharedString::from(id.to_owned()), safe),
        cx,
    )
    .image_source(|_| blocked())
    .on_link_click(|url, _, _, cx| {
        guarded("open a link", || open_web(url, cx));
    })
}

/// The Aegis prose style, shared by replies and Markdown files.
pub fn styled(view: TextView, cx: &App) -> TextView {
    let theme = theme(cx);
    let colors = palette(cx);
    let body = &theme.typography.desktop.body;
    let code = &theme.typography.desktop.code;
    let block = StyleRefinement::default()
        .bg(hsla(colors.surface.raised))
        .rounded(r(theme.radius.md))
        .border_t_1()
        .border_color(hsla(colors.overlay.highlight))
        .px(r(theme.space.s16))
        .py(r(theme.space.s12))
        .font_family(crate::ui::style::family(code.font))
        .text_size(r(12.))
        .line_height(r(18.))
        .text_color(hsla(colors.text.secondary))
        .shadow(crate::ui::style::shadows(&theme.shadow.light.card));
    let inline = StyleRefinement::default()
        .font_family(crate::ui::style::family(code.font))
        .text_size(r(body.size * 0.9))
        .bg(hsla(colors.surface.sunk))
        .rounded(r(theme.radius.xs))
        .px(px(5.))
        .py(px(1.));
    let style = TextViewStyle::default()
        .with_foreground(hsla(colors.text.primary))
        .with_muted_foreground(hsla(colors.text.secondary))
        .with_link(hsla(colors.text.primary))
        .with_selection(hsla(colors.state.pressed))
        .with_code_background(hsla(colors.surface.sunk))
        .with_border(hsla(colors.line.base))
        .with_code_block(block)
        .with_inline_code_style(inline);
    let motion = &theme.motion;
    view.style(style)
        .font_family(crate::ui::style::family(body.font))
        .text_size(r(body.size))
        .line_height(r(body.line_height))
        .text_color(hsla(colors.text.primary))
        .motion(
            TextViewMotion::default()
                .with_stream_fade(Duration::from_secs_f32(motion.duration.base / 1000.))
                .with_stream_fade_stagger(Duration::from_millis(34)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a reader sees after the rewrite: text, and links with their text and address.
    #[derive(Debug, Clone, PartialEq)]
    enum Shown {
        Text(String),
        Link(String, String),
    }

    fn shown(reply: &str) -> Vec<Shown> {
        fn walk(node: &Node, out: &mut Vec<Shown>) {
            match node {
                Node::Link(link) => out.push(Shown::Link(node.to_string(), link.url.clone())),
                Node::Text(text) => match out.last_mut() {
                    Some(Shown::Text(last)) => last.push_str(&text.value),
                    _ => out.push(Shown::Text(text.value.clone())),
                },
                Node::Html(_) | Node::Image(_) | Node::ImageReference(_) => {
                    panic!("drawn as is: {node:?}")
                }
                _ => node
                    .children()
                    .into_iter()
                    .flatten()
                    .for_each(|child| walk(child, out)),
            }
        }
        let safe = sanitize(reply);
        let tree = markdown::to_mdast(&safe, &ParseOptions::gfm()).unwrap();
        let mut out = Vec::new();
        walk(&tree, &mut out);
        out
    }

    fn text(value: &str) -> Shown {
        Shown::Text(value.into())
    }

    fn linked(text: &str, url: &str) -> Shown {
        Shown::Link(text.into(), url.into())
    }

    #[test]
    fn formats_markdown() {
        let reply = "**Done.** Two roles:\n\n- Sabenza IT\n- Takealot";
        assert_eq!(sanitize(reply), reply, "ordinary Markdown reads as written");
    }

    #[test]
    fn keeps_tables_and_their_links() {
        let reply =
            "| Business | Website |\n| --- | ---: |\n| Example company | https://example.com |";
        assert_eq!(sanitize(reply), reply);
        assert_eq!(
            shown(reply),
            [
                text("BusinessWebsiteExample company"),
                linked("https://example.com", "https://example.com")
            ]
        );
    }

    #[test]
    fn never_renders_html_and_drops_script_links() {
        let safe = sanitize(
            "<img src=x onerror=\"alert(1)\"> ![x](https://example.com/a.png) [click](javascript:alert(1)) \
             [page](data:text/html,hi) [box](vbscript:msgbox(1)) <script>alert(2)</script>",
        );
        for banned in ["<img", "<script", "javascript:", "data:", "vbscript:"] {
            assert!(!safe.contains(banned), "{banned} in {safe:?}");
        }
    }

    #[test]
    fn opens_links_outside_the_app() {
        assert_eq!(
            shown("See https://pnet.co.za/jobs"),
            [
                text("See "),
                linked("https://pnet.co.za/jobs", "https://pnet.co.za/jobs")
            ]
        );
    }

    #[test]
    fn shows_a_web_image_as_a_link_to_it_and_anything_that_cant_open_as_plain_text() {
        assert_eq!(
            shown(
                "![Listing](https://example.com/a.png) ![](https://example.com/b.png) ![Saved](shot.png) \
                 [![Badge](https://example.com/c.svg)](https://github.com/x) [Run it](javascript:alert(1)) \
                 [Email me](mailto:you@example.com)"
            ),
            [
                linked("Listing", "https://example.com/a.png"),
                text(" "),
                linked("https://example.com/b.png", "https://example.com/b.png"),
                text(" Saved "),
                linked("Badge", "https://github.com/x"),
                text(" Run it Email me"),
            ]
        );
    }

    #[test]
    fn never_leaves_a_link_without_text_when_its_image_has_no_alt_text() {
        // The last image's address is dropped as unsafe, so its link reads as its own address.
        assert_eq!(
            shown(
                "[![](https://example.com/badge.svg)](https://github.com/x) [![](badge.png)](https://github.com/y) \
                 [![](data:image/png;base64,AAAA)](https://github.com/z)"
            ),
            [
                linked("https://example.com/badge.svg", "https://github.com/x"),
                text(" "),
                linked("badge.png", "https://github.com/y"),
                text(" "),
                linked("https://github.com/z", "https://github.com/z"),
            ]
        );
    }

    #[test]
    fn blocks_non_web_links() {
        // A bare email and a reference to an unsafe target show as text, and nothing turns them back into links.
        assert_eq!(
            shown("Write to you@example.com or [run][go].\n\n[go]: javascript:alert(1)"),
            [text("Write to you@example.com or run.")]
        );
        assert!(web("HTTPS://example.com") && web("http:x"));
        assert!(!web("mailto:you@example.com") && !web("file:///C:/x") && !web("javascript:x"));
    }
}
