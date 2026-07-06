//! Shared blob-level diffing for history and staged extraction (RFC 0004
//! R3–R4): line counts plus a compact hunk-based patch rendering, with git's
//! NUL-byte binary heuristic.

use gix::diff::blob::{Algorithm, Diff, InternedInput};

/// Outcome of diffing two blob versions.
pub(crate) struct BlobDiff {
    /// Lines added.
    pub insertions: usize,
    /// Lines removed.
    pub deletions: usize,
    /// Whether either side looks binary; counts are 0 and no text is produced.
    pub binary: bool,
    /// Hunk text (`@@` headers with `-`/`+` lines); `None` if not requested,
    /// binary, or empty.
    pub text: Option<String>,
}

/// First-8000-bytes NUL heuristic, as used by git itself.
fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|&byte| byte == 0)
}

/// Diff two blob versions; `want_text` controls whether hunk text is rendered.
pub(crate) fn diff_blobs(old: &[u8], new: &[u8], want_text: bool) -> BlobDiff {
    if is_binary(old) || is_binary(new) {
        return BlobDiff {
            insertions: 0,
            deletions: 0,
            binary: true,
            text: want_text.then(|| "Binary files differ\n".to_owned()),
        };
    }

    let input = InternedInput::new(old, new);
    let diff = Diff::compute(Algorithm::Histogram, &input);
    let insertions = diff.count_additions() as usize;
    let deletions = diff.count_removals() as usize;

    let text = (want_text && insertions + deletions > 0).then(|| {
        use std::fmt::Write as _;
        let mut out = String::new();
        for hunk in diff.hunks() {
            let before = hunk.before.clone();
            let after = hunk.after.clone();
            let _ = writeln!(
                out,
                "@@ -{},{} +{},{} @@",
                before.start + 1,
                before.len(),
                after.start + 1,
                after.len(),
            );
            for &token in &input.before[before.start as usize..before.end as usize] {
                push_line(&mut out, '-', input.interner[token]);
            }
            for &token in &input.after[after.start as usize..after.end as usize] {
                push_line(&mut out, '+', input.interner[token]);
            }
        }
        out
    });

    BlobDiff {
        insertions,
        deletions,
        binary: false,
        text,
    }
}

/// Append one diff line with its sign, normalizing the line terminator.
fn push_line(out: &mut String, sign: char, line: &[u8]) {
    out.push(sign);
    let text = String::from_utf8_lossy(line);
    out.push_str(text.trim_end_matches(['\n', '\r']));
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_text_for_a_simple_change() {
        let diff = diff_blobs(b"one\ntwo\n", b"one\nthree\n", true);
        assert_eq!(diff.insertions, 1);
        assert_eq!(diff.deletions, 1);
        assert!(!diff.binary);
        let text = diff.text.expect("text requested");
        assert!(text.contains("-two"), "got: {text}");
        assert!(text.contains("+three"), "got: {text}");
        assert!(text.contains("@@"), "got: {text}");
    }

    #[test]
    fn binary_detection_short_circuits() {
        let diff = diff_blobs(b"a\0b", b"text\n", true);
        assert!(diff.binary);
        assert_eq!(diff.insertions, 0);
        assert_eq!(diff.deletions, 0);
        assert_eq!(diff.text.as_deref(), Some("Binary files differ\n"));
    }

    #[test]
    fn no_text_when_not_requested() {
        let diff = diff_blobs(b"a\n", b"b\n", false);
        assert!(diff.text.is_none());
        assert_eq!(diff.insertions, 1);
    }
}
