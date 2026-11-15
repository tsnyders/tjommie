//! Port of `packages/ui/lib/attachments.ts`: the files a message can carry, and a file's name from its path.

/// The most files one `chat.send` takes.
pub const MAX_FILES: usize = 20;

/// Adds the files the person picked or dropped, once each. A dropped item that isn't a file on disk arrives with an
/// empty path (`webUtils.getPathForFile`), and it's skipped. `None` when the message would carry too many.
pub fn add_files(current: &[String], incoming: &[String]) -> Option<Vec<String>> {
    let mut next = current.to_vec();
    for path in incoming {
        if !path.is_empty() && !next.contains(path) {
            next.push(path.clone());
        }
    }
    (next.len() <= MAX_FILES).then_some(next)
}

/// The file's name from its path, Windows or POSIX separators.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_picked_or_dropped_files_once_each_and_skips_dropped_items_that_arent_files() {
        assert_eq!(
            add_files(
                &[r"C:\a.pdf".to_owned()],
                &[
                    r"C:\b.pdf".to_owned(),
                    r"C:\a.pdf".to_owned(),
                    String::new()
                ]
            ),
            Some(vec![r"C:\a.pdf".to_owned(), r"C:\b.pdf".to_owned()])
        );
    }

    #[test]
    fn refuses_to_go_over_the_daemons_limit_of_20_files() {
        let twenty: Vec<String> = (0..MAX_FILES)
            .map(|index| format!(r"C:\f{index}.txt"))
            .collect();
        assert_eq!(
            add_files(&twenty[..19], &[r"C:\f19.txt".to_owned()]),
            Some(twenty.clone())
        );
        assert_eq!(add_files(&twenty, &[r"C:\extra.txt".to_owned()]), None);
    }

    #[test]
    fn shows_a_files_name_from_its_windows_path() {
        assert_eq!(
            file_name(r"C:\Users\owner\Documents\CV final.pdf"),
            "CV final.pdf"
        );
        assert_eq!(file_name("/home/owner/cv.pdf"), "cv.pdf");
    }
}
