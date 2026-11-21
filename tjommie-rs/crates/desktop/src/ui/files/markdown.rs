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

/// Opens `url` in the default browser if it is a web link; any other target does nothing. Windows gets the address
/// `links::open_external` parsed and normalised, never the raw text, so `http:x` or `https://` with no host doesn't
/// open (M4 review B1).
pub fn open_web(url: &str) {
    opens(url, desktop_host::links::open_external);
}

/// [`open_web`] with the opener passed in, for the tests.
fn opens(url: &str, open: impl FnOnce(&str) -> Result<(), String>) {
    if !web(url) {
        return;
    }
    if let Err(why) = open(url) {
        log::warn!("A link in a reply didn't open: {why}");
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

/// Deeper than prose nests: quotes, list items and footnote definitions opened at a line's start. Past it a reply is
/// shown as plain text, which nests nothing.
const MAX_NESTING: usize = 32;
/// The widest indent prose uses, in columns (a tab is to the next multiple of 4).
const MAX_INDENT: usize = 256;
/// The most that emphasis and brackets may bound their depth at (see [`inline_too_deep`]). The bound is loose for pairs
/// that close one after another in one paragraph, so it sits well above prose; the parser and gpui-kit's walk run out
/// of stack only in the thousands.
const MAX_INLINE: usize = 128;

/// Whether `text` nests deeper than the limits above, found before it is parsed for drawing. The parser, the rewrite
/// and gpui-kit's own walk of the result all recurse once a level, on the UI thread: a few thousand levels were a stack
/// overflow, which no guard catches (M4 review C1). Every line's containers are counted first, code or not. With those
/// within the limits, the parser can lay out the blocks with everything that nests inside a paragraph switched off: what
/// is code and what is a paragraph is then its own answer, where a guess that disagreed with it was a way past the
/// measure (M4 re-review R1). A code block holding a line past the container limits shows the reply as plain text.
fn too_deep(text: &str) -> bool {
    // A line ends at `\n`, `\r` or both, as the parser has it.
    if text.split(['\n', '\r']).any(block_nesting) {
        return true;
    }
    // Every marker in the text bounds each paragraph's own: under the limit, no paragraph can pass it, and the parse
    // that tells paragraphs from code isn't needed.
    if !inline_too_deep(text, 0..text.len(), &[]) {
        return false;
    }
    let mut blocks = ParseOptions::gfm();
    let constructs = &mut blocks.constructs;
    constructs.attention = false;
    constructs.gfm_strikethrough = false;
    constructs.label_start_link = false;
    constructs.label_start_image = false;
    constructs.label_end = false;
    constructs.gfm_label_start_footnote = false;
    match markdown::to_mdast(text, &blocks) {
        Ok(tree) => inline_too_deep_in(&tree, text),
        Err(_) => true,
    }
}

/// Whether any paragraph, heading or table cell under `node` (the only places emphasis and links are read) could nest
/// past [`MAX_INLINE`]. One with no place in `text` counts as too deep.
fn inline_too_deep_in(node: &Node, text: &str) -> bool {
    match node {
        Node::Paragraph(_) | Node::Heading(_) | Node::TableCell(_) => {
            let Some(span) = place(node) else {
                return true;
            };
            let mut literal = Vec::new();
            literal_parts(node, &mut literal);
            literal.sort_by_key(|part| part.start);
            inline_too_deep(text, span, &literal)
        }
        _ => node
            .children()
            .into_iter()
            .flatten()
            .any(|child| inline_too_deep_in(child, text)),
    }
}

/// The code spans and HTML under `node`, which hold no emphasis or links.
fn literal_parts(node: &Node, into: &mut Vec<Range<usize>>) {
    for child in node.children().into_iter().flatten() {
        match child {
            Node::InlineCode(_) | Node::Html(_) => into.extend(place(child)),
            _ => literal_parts(child, into),
        }
    }
}

/// Where `node` is in the text, in bytes.
fn place(node: &Node) -> Option<Range<usize>> {
    node.position()
        .map(|position| position.start.offset..position.end.offset)
}

/// Emphasis by its marker (`*`, `_`, `~`), then brackets: a pair is only ever two of one kind.
const KINDS: usize = 4;

/// Whether the emphasis and brackets in `span` of `text` (one paragraph, heading or cell, less its `literal` parts)
/// could nest past [`MAX_INLINE`]. A pair that encloses a point has its opener before the point and its closer after
/// it, so the depth there is at most the markers that could open before it against those that could close after it,
/// kind by kind. Nothing is taken off for a closer on the way: an escape or a link's address makes the parser ignore a
/// marker this scan may still see, and a closer it fakes would hide real openers (M4 re-review R1).
fn inline_too_deep(text: &str, span: Range<usize>, literal: &[Range<usize>]) -> bool {
    let mut after = [0usize; KINDS];
    runs(text, span.clone(), literal, |kind, _, closes| {
        if let Some(after) = after.get_mut(kind) {
            *after += closes;
        }
    });
    let (mut before, mut deep) = ([0usize; KINDS], false);
    runs(text, span, literal, |kind, opens, closes| {
        let bound: usize = before.iter().zip(&after).map(|(o, c)| *o.min(c)).sum();
        deep |= bound > MAX_INLINE;
        if let (Some(before), Some(after)) = (before.get_mut(kind), after.get_mut(kind)) {
            *before += opens;
            *after = after.saturating_sub(closes);
        }
    });
    deep
}

/// Each emphasis run and bracket in `span` of `text` outside its `literal` parts (in order, apart), as `(kind, markers
/// that could open, markers that could close)`. A backslash before punctuation escapes it, as in the parser, and a run
/// is classed by the characters beside it as written, past the span's ends too, as the parser classes it.
fn runs(
    text: &str,
    span: Range<usize>,
    literal: &[Range<usize>],
    mut each: impl FnMut(usize, usize, usize),
) {
    let Some(inside) = text.get(span.clone()) else {
        return;
    };
    let mut before = text
        .get(..span.start)
        .and_then(|head| head.chars().next_back());
    let mut chars = inside.char_indices().peekable();
    let mut parts = literal.iter().peekable();
    while let Some((at, c)) = chars.next() {
        let at = span.start + at;
        while parts.next_if(|part| part.end <= at).is_some() {}
        let counted = !parts.peek().is_some_and(|part| part.contains(&at));
        if let ('\\', Some(&(_, next))) = (c, chars.peek())
            && next.is_ascii_punctuation()
        {
            chars.next();
            before = Some(next);
            continue;
        }
        match c {
            '*' | '_' | '~' => {
                let mut size = 1;
                while chars.next_if(|&(_, m)| m == c).is_some() {
                    size += 1;
                }
                // A marker is one byte.
                let after = text.get(at + size..).and_then(|tail| tail.chars().next());
                let (opens, closes) = flanks(c, before, after);
                let kind = match c {
                    '*' => 0,
                    '_' => 1,
                    _ => 2,
                };
                if counted {
                    each(
                        kind,
                        if opens { size } else { 0 },
                        if closes { size } else { 0 },
                    );
                }
            }
            '[' if counted => each(3, 1, 0),
            ']' if counted => each(3, 0, 1),
            _ => {}
        }
        before = Some(c);
    }
}

/// How the parser classes a character beside an emphasis run.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    Whitespace,
    Punctuation,
    Other,
}

/// The classes `c` can have for the parser (markdown-rs `classify`). Outside ASCII it tells punctuation from the rest by
/// a table this crate can't read, so anything there that isn't whitespace is tried both ways.
fn classes(c: Option<char>) -> &'static [Class] {
    match c {
        None => &[Class::Whitespace],
        Some(c) if c.is_whitespace() => &[Class::Whitespace],
        Some(c) if c.is_ascii_punctuation() => &[Class::Punctuation],
        Some(c) if c.is_ascii() => &[Class::Other],
        Some(_) => &[Class::Punctuation, Class::Other],
    }
}

/// Whether a run of `marker` between `before` and `after` (the characters beside it as written, `None` at a line's
/// ends) could open and could close emphasis, by the parser's rules (markdown-rs `attention::get_sequences`).
fn flanks(marker: char, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let (mut opens, mut closes) = (false, false);
    for &b in classes(before) {
        for &a in classes(after) {
            let open = a == Class::Other
                || (a == Class::Punctuation && b != Class::Other)
                || (marker != '~' && matches!(after, Some('*' | '_' | '~')));
            let close = b == Class::Other
                || (b == Class::Punctuation && a != Class::Other)
                || (marker != '~' && matches!(before, Some('*' | '_' | '~')));
            if marker == '_' {
                opens |= open && (b != Class::Other || !close);
                closes |= close && (a != Class::Other || !open);
            } else {
                opens |= open;
                closes |= close;
            }
        }
    }
    (opens, closes)
}

/// The containers a line opens at its start (`>` quotes, list markers and footnote definitions) and its indent,
/// against the limits.
fn block_nesting(line: &str) -> bool {
    let (mut depth, mut columns) = (0usize, 0usize);
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        let mut ahead = chars.clone();
        ahead.next();
        let spaced = matches!(ahead.peek(), Some(' ' | '\t'));
        match c {
            ' ' => columns += 1,
            '\t' => columns += 4 - columns % 4,
            '>' => {
                depth += 1;
                columns += 1;
            }
            '-' | '*' | '+' if spaced => {
                depth += 1;
                columns += 1;
            }
            '[' if ahead.peek() == Some(&'^') => {
                // A footnote definition, `[^label]:`, holds blocks as a list item does (M4 re-review R1).
                let label = ahead.clone().take_while(|c| *c != ']').count();
                let mut rest = ahead.clone().skip(label);
                if !(rest.next() == Some(']') && rest.next() == Some(':')) {
                    break;
                }
                depth += 1;
                columns += label + 3;
                // The `[`, the label and its `]`; the `:` is taken below.
                for _ in 0..label + 2 {
                    chars.next();
                }
            }
            '0'..='9' => {
                // An ordered marker: up to nine digits, then `.` or `)`, then a space.
                let digits = chars.clone().take_while(char::is_ascii_digit).count();
                let mut rest = chars.clone().skip(digits);
                let marker = digits <= 9
                    && matches!(rest.next(), Some('.' | ')'))
                    && matches!(rest.next(), Some(' ' | '\t'));
                if !marker {
                    break;
                }
                depth += 1;
                columns += digits + 1;
                for _ in 0..digits {
                    chars.next();
                }
            }
            _ => break,
        }
        chars.next();
        if depth > MAX_NESTING || columns > MAX_INDENT {
            return true;
        }
    }
    false
}

/// The reply as Markdown that is safe to draw. A reply that nests too deeply or doesn't parse is shown as its plain
/// text.
pub fn sanitize(text: &str) -> String {
    if too_deep(text) {
        return plain(text);
    }
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
    document(SharedString::from(id.to_owned()), safe, cx)
}

/// Markdown already rewritten by [`sanitize`], drawn as prose: web links open in the default browser, images never load.
pub fn document(id: SharedString, safe: SharedString, cx: &App) -> TextView {
    styled(TextView::markdown(id, safe), cx)
        .image_source(|_| blocked())
        .on_link_click(|url, _, _, _| {
            guarded("open a link", || open_web(url));
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

    /// M4 review B1: a link opens through `links::open_external`, which parses and normalises it; one with no host
    /// (`http:x`, `https://`) never reaches Windows, and anything but the web isn't even tried.
    #[test]
    fn a_link_opens_only_through_the_normaliser() {
        let opened = std::cell::RefCell::new(Vec::new());
        let open = |url: &str| {
            desktop_host::links::open_external_with(url, |normal| {
                opened.borrow_mut().push(normal.to_owned());
                Ok(())
            })
        };
        for url in [
            "http:x",
            "https://",
            "mailto:you@example.com",
            "HTTPS://Example.com/a b",
        ] {
            opens(url, open);
        }
        assert_eq!(*opened.borrow(), ["https://example.com/a%20b"]);
    }

    fn depth(node: &Node) -> usize {
        1 + node
            .children()
            .into_iter()
            .flatten()
            .map(depth)
            .max()
            .unwrap_or(0)
    }

    /// Each of `deep` is shown as plain text, in bounded time, and what's left nests nothing.
    fn all_plain(deep: &[(&str, String)]) {
        for (case, deep) in deep {
            let started = std::time::Instant::now();
            let safe = sanitize(deep);
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{case}: measured fast"
            );
            assert_eq!(safe, plain(deep), "{case}");
            let tree = markdown::to_mdast(&safe, &ParseOptions::gfm()).unwrap();
            assert!(depth(&tree) <= 4, "{case}: nests nothing");
        }
    }

    /// M4 review C1: nesting past the limit (quotes, lists, emphasis, brackets or indent) is shown as plain text, so
    /// neither the parser nor gpui-kit's walk can recurse deep.
    #[test]
    fn a_reply_nested_past_the_limit_is_plain_text() {
        all_plain(&[
            ("quoted", format!("{}hello", ">".repeat(50_000))),
            ("listed", format!("{}item", "- ".repeat(20_000))),
            (
                "emphasised",
                format!("{}x{}", "*a ".repeat(20_000), " a*".repeat(20_000)),
            ),
            (
                "bracketed",
                format!("{}x{}", "[".repeat(20_000), "]".repeat(20_000)),
            ),
            ("indented", format!("{}x", " ".repeat(MAX_INDENT + 1))),
        ]);
        // Within the limits Markdown still reads as Markdown; a code block holding a line past them doesn't.
        let nested = format!("{}quoted", "> ".repeat(MAX_NESTING));
        assert_ne!(sanitize(&nested), plain(&nested));
        let code = format!("```\n{}deep code\n```", " ".repeat(MAX_INDENT + 10));
        assert_eq!(sanitize(&code), plain(&code));
        assert_ne!(
            sanitize("**bold** and *em* and [a](https://x.example)"),
            plain("**bold**")
        );
    }

    /// M4 re-review R1: the measure can't be stepped around. A to D are lines that look like a fence but aren't one to
    /// the parser; E nests emphasis opened after punctuation. The rest each defeated a count that takes a closer off:
    /// an escaped marker, a closer in a code span, a closer of another kind, a closer escaped beside a marker; nested
    /// footnote definitions and a lone `\r` line ending defeated the line measure. Each nests about 300 deep at 300.
    /// Those four with closers in them run at 5 000: the parser itself takes seconds over 50 000 escapes or code spans
    /// in one paragraph, deep or not.
    #[test]
    fn the_measure_cant_be_stepped_around() {
        let (n, m) = (50_000, 5_000);
        let quotes = ">".repeat(n);
        all_plain(&[
            ("A, an indented fence", format!("    ```\n{quotes}hello")),
            (
                "B, a backtick in the info string",
                format!("``` a`b\n{quotes}hello"),
            ),
            (
                "C, a fence in a list item",
                format!("- item\n  ```\n{quotes}hello"),
            ),
            ("D, a fence in HTML", format!("<div>\n```\n\n{quotes}hello")),
            ("E, *", format!("{}x{}", "(*a".repeat(n), "*)".repeat(n))),
            ("E, _", format!("{}x{}", "(_a".repeat(n), "_)".repeat(n))),
            (
                "an escape",
                format!("{}x{}", "(*a\\* ".repeat(m), "*)".repeat(m)),
            ),
            (
                "a code span",
                format!("{}x{}", "(*a`a* `".repeat(m), "*)".repeat(m)),
            ),
            (
                "another kind",
                format!("{}x{}", "(*a_)".repeat(m), "*)".repeat(m)),
            ),
            (
                "an escaped neighbour",
                format!("{}x{}", "(*a".repeat(m), "\\**a".repeat(m)),
            ),
            ("footnotes", format!("{}x", "[^a]: ".repeat(n))),
            ("a lone CR", format!("x\r{quotes}hello")),
        ]);
    }

    /// What prose and code hold stays Markdown: long lists and tables of bold labels, and emphasis in code.
    #[test]
    fn long_lists_tables_and_code_keep_their_markdown() {
        let python = "def f(*args, **kwargs):\n    return my_func(x**2, *args)\n".repeat(100);
        for text in [
            "- **Item**: text\n".repeat(200),
            (1..=100)
                .map(|n| format!("{n}. **Item**: text\n"))
                .collect(),
            format!("| a | b |\n|---|---|\n{}", "| **x** | *y* |\n".repeat(300)),
            format!("```python\n{python}```\n"),
            "Some *emphasis* and **strong** words, _here_ and __there__. ".repeat(40),
        ] {
            assert_ne!(sanitize(&text), plain(&text), "{text:.40}");
        }
    }
}
