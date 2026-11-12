//! Reading a `--filter` command's answer.

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Answer {
    Send,
    Drop,
    /// The filter failed; the batch is sent anyway and the failure reported.
    Error,
}

/// `stdout` is what the filter printed; `exit` is its exit code, `None` when
/// it was killed (timeout, signal).
///
/// A printed `true`/`yes` or `false`/`no` (case-insensitive, last non-empty
/// line) decides. Otherwise exit 0 sends and exit 1 drops. Anything else is
/// an error.
pub fn answer(stdout: &str, exit: Option<i32>) -> Answer {
    let Some(code) = exit else {
        return Answer::Error;
    };
    let last = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_ascii_lowercase);
    match last.as_deref() {
        Some("true" | "yes") => return Answer::Send,
        Some("false" | "no") => return Answer::Drop,
        _ => {}
    }
    match code {
        0 => Answer::Send,
        1 => Answer::Drop,
        _ => Answer::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_words_in_any_case() {
        assert_eq!(answer("TRUE\n", Some(0)), Answer::Send);
        assert_eq!(answer("Yes", Some(1)), Answer::Send);
        assert_eq!(answer("false\n", Some(0)), Answer::Drop);
        assert_eq!(answer("  No  \n\n", Some(0)), Answer::Drop);
    }

    #[test]
    fn last_non_empty_line_wins() {
        assert_eq!(
            answer("it looks like an error\nmaybe true\nfalse\n\n", Some(0)),
            Answer::Drop
        );
    }

    #[test]
    fn no_word_falls_back_to_exit_code() {
        assert_eq!(answer("", Some(0)), Answer::Send);
        assert_eq!(answer("thinking...", Some(1)), Answer::Drop);
    }

    #[test]
    fn other_exit_or_kill_is_an_error() {
        assert_eq!(answer("", Some(2)), Answer::Error);
        assert_eq!(answer("garbage", Some(127)), Answer::Error);
        assert_eq!(answer("true", None), Answer::Error);
    }
}
