//! Bytes from a pipe or a file into clean lines.

use std::sync::OnceLock;

use regex::Regex;

use super::MAX_LINE_BYTES;

/// Splits a byte stream into lines. A last line without a newline is held
/// until the newline arrives or the caller releases it with [`take_partial`].
///
/// [`take_partial`]: LineSplitter::take_partial
#[derive(Default)]
pub struct LineSplitter {
    buf: Vec<u8>,
    /// Bytes of the current line already dropped: a line that never ends
    /// must not grow without bound (it is cut to [`MAX_LINE_BYTES`] anyway).
    over: usize,
}

/// What is kept of a line without its newline: a little more than is shown,
/// so the cut stays on a char boundary.
const KEEP: usize = MAX_LINE_BYTES + 8;

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            out.push(clean_over(&line[..line.len() - 1], self.over));
            self.over = 0;
        }
        if self.buf.len() > KEEP {
            self.over += self.buf.len() - KEEP;
            self.buf.truncate(KEEP);
        }
        out
    }

    pub fn has_partial(&self) -> bool {
        !self.buf.is_empty()
    }

    /// Releases a held last line (the caller waited long enough for its newline).
    pub fn take_partial(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let line = std::mem::take(&mut self.buf);
        Some(clean_over(&line, std::mem::take(&mut self.over)))
    }
}

fn ansi() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[@-Z\\-_]")
            .expect("ansi regex")
    })
}

/// One raw line into text: lossy UTF-8, no ANSI escapes, no trailing `\r`,
/// cut at [`MAX_LINE_BYTES`] with a marker.
pub fn clean(raw: &[u8]) -> String {
    clean_over(raw, 0)
}

/// [`clean`] for a line of which `over` more bytes were already dropped.
fn clean_over(raw: &[u8], over: usize) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = ansi().replace_all(&text, "");
    let text = text.trim_end_matches('\r');
    if over == 0 {
        return cut(text, MAX_LINE_BYTES);
    }
    let mut end = text.len().min(MAX_LINE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} … [cut {} bytes]", &text[..end], text.len() - end + over)
}

/// Cuts `text` to at most `max` bytes on a char boundary, with a marker.
pub fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} … [cut {} bytes]", &text[..end], text.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_last_line_is_held_then_released() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"one\ntw"), vec!["one"]);
        assert!(s.has_partial());
        assert_eq!(s.push(b"o\n"), vec!["two"]);
        assert!(!s.has_partial());
        s.push(b"three");
        assert_eq!(s.take_partial().as_deref(), Some("three"));
        assert_eq!(s.take_partial(), None);
    }

    #[test]
    fn crlf_and_many_lines_in_one_chunk() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"a\r\nb\n\nc\n"), vec!["a", "b", "", "c"]);
    }

    #[test]
    fn long_line_is_cut_with_marker() {
        let line = "x".repeat(MAX_LINE_BYTES + 10);
        let out = clean(line.as_bytes());
        assert!(out.starts_with(&"x".repeat(MAX_LINE_BYTES)));
        assert!(out.ends_with("[cut 10 bytes]"), "{out}");
    }

    #[test]
    fn cut_respects_char_boundary() {
        assert_eq!(cut("ééé", 3), "é … [cut 4 bytes]");
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        assert_eq!(clean(b"ok \xff end"), "ok \u{fffd} end");
    }

    #[test]
    fn crlf_with_colour_and_lone_cr() {
        let mut s = LineSplitter::default();
        assert_eq!(
            s.push(b"\x1b[31mred\x1b[0m\r\nmid\rdle\r\r\n"),
            vec!["red", "mid\rdle"]
        );
    }

    #[test]
    fn binary_bytes_never_break_a_line() {
        let mut s = LineSplitter::default();
        let out = s.push(b"\x00\x01\xfe\xff bin\n\xe2\x82\n");
        assert_eq!(out, vec!["\u{0}\u{1}\u{fffd}\u{fffd} bin", "\u{fffd}"]);
    }

    #[test]
    fn whitespace_lines_are_kept() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"  \n\t\n"), vec!["  ", "\t"]);
    }

    #[test]
    fn exactly_max_is_not_cut() {
        let line = "y".repeat(MAX_LINE_BYTES);
        assert_eq!(clean(line.as_bytes()), line);
    }

    #[test]
    fn a_line_that_never_ends_stays_bounded() {
        let mut s = LineSplitter::default();
        let chunk = vec![b'z'; 64 * 1024];
        for _ in 0..64 {
            assert!(s.push(&chunk).is_empty());
            assert!(s.buf.len() <= KEEP);
        }
        let out = s.push(b"tail\nnext\n");
        assert_eq!(out.len(), 2);
        let total = 64 * 64 * 1024 + 4;
        assert!(
            out[0].ends_with(&format!("[cut {} bytes]", total - MAX_LINE_BYTES)),
            "{}",
            &out[0][MAX_LINE_BYTES..]
        );
        assert_eq!(out[1], "next");
        // The same for a partial line released at exit.
        s.push(&chunk);
        let p = s.take_partial().unwrap();
        assert!(p.ends_with(&format!("[cut {} bytes]", 64 * 1024 - MAX_LINE_BYTES)));
        assert!(!s.has_partial());
    }

    #[test]
    fn ansi_escapes_are_removed() {
        assert_eq!(clean(b"\x1b[31mERROR\x1b[0m boom"), "ERROR boom");
        assert_eq!(clean(b"\x1b]0;title\x07text"), "text");
    }
}
