//! Port of `packages/ui/lib/markdown-document.ts` (T1.4): a Markdown document read in 32 KiB sections of its bytes, each cut
//! at UTF-8 character boundaries, and a search that finds text across a section boundary. Each section is fetched on its
//! own, so a 200 MiB file is never assembled. Positions are byte offsets, as the TypeScript's are.

use protocol::attachments::MAX_ATTACHMENT_BYTES;

use super::document_ranges::{DocumentError, RangeFetch, Signal, document_range};

pub const MARKDOWN_SECTION_BYTES: u64 = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownSection {
    pub text: String,
    pub begin: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownMatch {
    pub offset: u64,
    pub section: u64,
}

fn is_continuation(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}

fn as_u64(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// `markdownSection`: section `section` of a `size`-byte document, as text. Its bytes are fetched with up to three bytes of
/// the section before, so a character split at its start is whole; a character split at its end goes to the next section.
pub async fn markdown_section<F: RangeFetch>(
    fetch: &F,
    url: &str,
    size: u64,
    section: u64,
    signal: &Signal,
) -> Result<MarkdownSection, DocumentError> {
    signal.throw_if_aborted()?;
    let sections = size.div_ceil(MARKDOWN_SECTION_BYTES).max(1);
    if size > MAX_ATTACHMENT_BYTES || section >= sections {
        return Err(DocumentError::InvalidSection);
    }
    if size == 0 {
        return Ok(MarkdownSection {
            text: String::new(),
            begin: 0,
            end: 0,
        });
    }
    let start = section * MARKDOWN_SECTION_BYTES;
    let end = size.min(start + MARKDOWN_SECTION_BYTES);
    let first = start.saturating_sub(3);
    let bytes = document_range(fetch, url, size, first, end, signal).await?;
    let at = |index: usize| bytes.get(index).copied().unwrap_or(0);

    let mut begin_at = usize::try_from(start - first).unwrap_or(0);
    let mut end_at = bytes.len();
    while begin_at > 0 && is_continuation(at(begin_at)) {
        begin_at -= 1;
    }
    if end < size {
        // A final incomplete character belongs to the next section.
        let mut last = end_at.saturating_sub(1);
        while last > 0 && is_continuation(at(last)) {
            last -= 1;
        }
        let lead = at(last);
        let width = if lead < 0x80 {
            1
        } else if lead < 0xe0 {
            2
        } else if lead < 0xf0 {
            3
        } else {
            4
        };
        if end_at - last < width {
            end_at = last;
        }
    }
    let slice = bytes
        .get(begin_at..end_at)
        .ok_or(DocumentError::InvalidSection)?;
    let text = std::str::from_utf8(slice)
        .map_err(|_| DocumentError::NotUtf8)?
        .to_owned();
    Ok(MarkdownSection {
        text,
        begin: first + as_u64(begin_at),
        end: first + as_u64(end_at),
    })
}

/// The last `units` UTF-16 code units of `text` (JavaScript's `slice(-units)`), as whole characters.
fn last_utf16_units(text: &str, units: usize) -> &str {
    let mut taken = 0;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        if taken >= units {
            break;
        }
        taken += character.len_utf16();
        start = index;
    }
    text.get(start..).unwrap_or("")
}

/// `findMarkdownText`: the first case-insensitive match of `query` at or after byte `from`, across sections. The last
/// `query.length - 1` UTF-16 units of each section are carried into the next, so a match crossing a boundary is found.
/// Lowercasing keeps the text's length for the text the app shows; a length-changing mapping would shift the offset, as it
/// does in the TypeScript.
pub async fn find_markdown_text<F: RangeFetch>(
    fetch: &F,
    url: &str,
    size: u64,
    query: &str,
    from: u64,
    signal: &Signal,
) -> Result<Option<MarkdownMatch>, DocumentError> {
    let query_units = query.encode_utf16().count();
    if query.is_empty() || query_units > 256 || from > size {
        return Err(DocumentError::InvalidSearch);
    }
    let needle = query.to_lowercase();
    let carry_units = query_units.saturating_sub(1);
    let mut carry = String::new();
    let mut carry_bytes = 0u64;
    for section in (from / MARKDOWN_SECTION_BYTES)..size.div_ceil(MARKDOWN_SECTION_BYTES) {
        signal.throw_if_aborted()?;
        let current = markdown_section(fetch, url, size, section, signal).await?;
        let combined = format!("{carry}{}", current.text);
        let lowered = combined.to_lowercase();
        let mut search = 0;
        while let Some(found) = lowered.get(search..).and_then(|rest| rest.find(&needle)) {
            let index = search + found;
            let before = combined.get(..index).map_or(index, str::len);
            let offset = current
                .begin
                .saturating_sub(carry_bytes)
                .saturating_add(as_u64(before));
            if offset >= from {
                return Ok(Some(MarkdownMatch {
                    offset,
                    section: offset / MARKDOWN_SECTION_BYTES,
                }));
            }
            let step = lowered
                .get(index..)
                .and_then(|rest| rest.chars().next())
                .map_or(1, char::len_utf8);
            search = index + step;
        }
        carry = last_utf16_units(&combined, carry_units).to_owned();
        carry_bytes = as_u64(carry.len());
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Mutex;

    use protocol::attachments::MAX_ATTACHMENT_BYTES;

    use super::super::document_ranges::RangeReply;
    use super::*;

    /// A document served in honest range replies, which remembers each range asked for. `body` gives the bytes.
    struct Serve<B> {
        size: u64,
        requests: Mutex<Vec<(u64, u64)>>,
        body: B,
    }

    impl<B: Fn(u64, u64) -> Vec<u8> + Sync> Serve<B> {
        fn new(size: u64, body: B) -> Self {
            Self {
                size,
                requests: Mutex::new(Vec::new()),
                body,
            }
        }

        fn asked(&self) -> Vec<(u64, u64)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl<B: Fn(u64, u64) -> Vec<u8> + Sync> RangeFetch for Serve<B> {
        fn fetch_range(
            &self,
            _url: &str,
            begin: u64,
            last: u64,
        ) -> impl Future<Output = Result<RangeReply, String>> + Send {
            self.requests.lock().unwrap().push((begin, last));
            let reply = RangeReply {
                status: 206,
                content_range: Some(format!("bytes {begin}-{last}/{}", self.size)),
                content_length: Some((last - begin + 1).to_string()),
                body: (self.body)(begin, last),
            };
            std::future::ready(Ok(reply))
        }
    }

    /// Answers 200 where a range was asked for: the server that ignores Range.
    struct IgnoresRange;

    impl RangeFetch for IgnoresRange {
        fn fetch_range(
            &self,
            _url: &str,
            _begin: u64,
            _last: u64,
        ) -> impl Future<Output = Result<RangeReply, String>> + Send {
            std::future::ready(Ok(RangeReply {
                status: 200,
                content_range: None,
                content_length: None,
                body: b"bad".to_vec(),
            }))
        }
    }

    fn section_bytes() -> u64 {
        MARKDOWN_SECTION_BYTES
    }

    #[tokio::test]
    async fn reads_first_and_last_sections_of_a_200mib_markdown_file_without_reading_or_assembling_the_intervening_file()
     {
        let server = Serve::new(MAX_ATTACHMENT_BYTES, |begin, last| {
            vec![97; (last - begin + 1) as usize]
        });
        let signal = Signal::new();
        let first = markdown_section(&server, "local", MAX_ATTACHMENT_BYTES, 0, &signal)
            .await
            .unwrap();
        assert_eq!(first.text.len(), section_bytes() as usize);
        let last = markdown_section(&server, "local", MAX_ATTACHMENT_BYTES, 6399, &signal)
            .await
            .unwrap();
        assert_eq!(last.text.len(), section_bytes() as usize);
        let lengths: Vec<u64> = server
            .asked()
            .iter()
            .map(|(begin, last)| last - begin + 1)
            .collect();
        assert_eq!(lengths, vec![section_bytes(), section_bytes() + 3]);
    }

    #[tokio::test]
    async fn preserves_utf_8_characters_across_every_paged_boundary_and_finds_text_crossing_a_boundary()
     {
        let section = section_bytes() as usize;
        let text = format!(
            "{}{}{}",
            "a".repeat(section - 1),
            "\u{1F600} caf\u{e9} boundary",
            "x".repeat(section)
        );
        let bytes = text.clone().into_bytes();
        let size = bytes.len() as u64;
        let server = Serve::new(size, move |begin, last| {
            bytes[begin as usize..=last as usize].to_vec()
        });
        let signal = Signal::new();
        let mut joined = String::new();
        for section_index in 0..size.div_ceil(section_bytes()) {
            joined.push_str(
                &markdown_section(&server, "local", size, section_index, &signal)
                    .await
                    .unwrap()
                    .text,
            );
        }
        assert_eq!(joined, text);
        let found =
            find_markdown_text(&server, "local", size, "a\u{1F600} caf\u{e9}", 0, &signal).await;
        assert_eq!(
            found,
            Ok(Some(MarkdownMatch {
                offset: section_bytes() - 2,
                section: 0
            }))
        );
        let boundary = find_markdown_text(&server, "local", size, "boundary", 0, &signal)
            .await
            .unwrap();
        assert_eq!(boundary.map(|found| found.section), Some(1));
    }

    #[tokio::test]
    async fn rejects_non_utf8_range_header_count_corruption_and_an_aborted_read() {
        let signal = Signal::new();
        let not_text = Serve::new(2, |_, _| vec![255, 255]);
        assert_eq!(
            markdown_section(&not_text, "local", 2, 0, &signal).await,
            Err(DocumentError::NotUtf8)
        );

        let error = document_range(&IgnoresRange, "local", 3, 0, 3, &signal)
            .await
            .unwrap_err();
        assert_eq!(error, DocumentError::Unavailable);
        assert!(error.to_string().contains("no longer available"));

        let aborted = Signal::new();
        aborted.abort();
        assert_eq!(
            markdown_section(&not_text, "local", 3, 0, &aborted).await,
            Err(DocumentError::Cancelled)
        );
    }

    #[tokio::test]
    async fn an_empty_document_is_one_empty_section_and_nothing_is_fetched() {
        let server = Serve::new(0, |_, _| Vec::new());
        let signal = Signal::new();
        let section = markdown_section(&server, "local", 0, 0, &signal)
            .await
            .unwrap();
        assert_eq!(
            (section.text.as_str(), section.begin, section.end),
            ("", 0, 0)
        );
        assert_eq!(
            markdown_section(&server, "local", 0, 1, &signal).await,
            Err(DocumentError::InvalidSection)
        );
        assert!(server.asked().is_empty());
    }

    #[tokio::test]
    async fn refuses_sections_and_searches_outside_the_document_before_fetching() {
        let server = Serve::new(3, |begin, last| vec![97; (last - begin + 1) as usize]);
        let signal = Signal::new();
        assert_eq!(
            markdown_section(&server, "local", 3, 1, &signal).await,
            Err(DocumentError::InvalidSection)
        );
        assert_eq!(
            markdown_section(&server, "local", MAX_ATTACHMENT_BYTES + 1, 0, &signal).await,
            Err(DocumentError::InvalidSection)
        );
        assert_eq!(
            find_markdown_text(&server, "local", 3, "", 0, &signal).await,
            Err(DocumentError::InvalidSearch)
        );
        assert_eq!(
            find_markdown_text(&server, "local", 3, &"a".repeat(257), 0, &signal).await,
            Err(DocumentError::InvalidSearch)
        );
        assert_eq!(
            find_markdown_text(&server, "local", 3, "a", 4, &signal).await,
            Err(DocumentError::InvalidSearch)
        );
        assert!(server.asked().is_empty());
    }

    #[tokio::test]
    async fn search_starts_at_the_given_offset_and_reports_no_match_as_none() {
        let server = Serve::new(6, |begin, last| {
            b"abcABC"[begin as usize..=last as usize].to_vec()
        });
        let signal = Signal::new();
        assert_eq!(
            find_markdown_text(&server, "local", 6, "abc", 0, &signal).await,
            Ok(Some(MarkdownMatch {
                offset: 0,
                section: 0
            }))
        );
        assert_eq!(
            find_markdown_text(&server, "local", 6, "ABC", 1, &signal).await,
            Ok(Some(MarkdownMatch {
                offset: 3,
                section: 0
            }))
        );
        assert_eq!(
            find_markdown_text(&server, "local", 6, "zzz", 0, &signal).await,
            Ok(None)
        );
    }
}
