//! Read-side Git access through gitoxide (RFC 0004).
//!
//! Everything here is pure-Rust and read-only: repository discovery, history
//! walking with per-commit context extraction (R1–R3), and staged-change
//! extraction (R4). Remote clones land in a later slice (R5); the single
//! write-side operation — creating a commit — deliberately lives elsewhere and
//! shells out to the system `git` (R6).

mod blobdiff;
pub mod commit;
pub mod remote;
pub mod repo;
pub mod staged;

pub use commit::{CommitError, CommitOutcome, create_commit};
pub use remote::{RemoteClone, clone_for_analysis, interrupt_clones};
pub use repo::{HistoryOptions, Repo};
pub use staged::staged_changes;

use serde::Serialize;

/// Marker appended whenever patch text is cut at a byte budget (RFC 0004 R3).
pub const TRUNCATION_MARKER: &str = "\n... [patch truncated]";

/// Errors from the git layer; every message is actionable (RFC 0004 R8).
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The working directory is not inside a Git repository.
    #[error("not inside a Git repository (searched upward from `{path}`)")]
    NotARepository {
        /// Directory the discovery started from.
        path: String,
    },
    /// The repository has no commits that can be analyzed.
    #[error("no commits to analyze (the history is empty or contains only merge commits)")]
    NoCommits,
    /// `--from` did not resolve to a commit.
    #[error(
        "cannot resolve revision `{spec}`; pass a commit SHA, branch, or tag that exists in this repository"
    )]
    BadRevision {
        /// The revision spec as given by the user.
        spec: String,
    },
    /// `write` was invoked with an empty index diff.
    #[error("nothing is staged; stage changes with `git add` before running `gitalyzer write`")]
    NothingStaged,
    /// Cloning a remote for analysis failed for non-credential reasons.
    #[error("cannot clone `{url}`: {message}")]
    CloneFailed {
        /// The URL as given.
        url: String,
        /// Underlying failure.
        message: String,
    },
    /// Cloning failed on authentication (RFC 0004 R9).
    #[error(
        "authentication failed while cloning `{url}`: {message}\n\
         gitalyzer uses your existing git credentials — check your SSH agent/config for SSH \
         URLs, or your git credential helper for HTTPS"
    )]
    CloneAuthFailed {
        /// The URL as given.
        url: String,
        /// Underlying failure.
        message: String,
    },
    /// Any unexpected failure inside gitoxide.
    #[error("git operation failed: {0}")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Wrap an arbitrary gitoxide error as [`GitError::Internal`].
pub(crate) fn internal(error: impl std::error::Error + Send + Sync + 'static) -> GitError {
    GitError::Internal(Box::new(error))
}

/// Aggregated change statistics (RFC 0004 R3).
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiffStat {
    /// Number of files touched.
    pub files_changed: usize,
    /// Total lines added.
    pub insertions: usize,
    /// Total lines removed.
    pub deletions: usize,
    /// Paths of the touched files, in diff order.
    pub files: Vec<String>,
}

/// Everything extracted per commit for analysis (RFC 0004 R3).
#[derive(Debug, Clone, Serialize)]
pub struct CommitInfo {
    /// Full hex object id.
    pub sha: String,
    /// Repository-aware short id.
    pub short_sha: String,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Authored date, ISO 8601.
    pub date: String,
    /// Full commit message (subject and body), trimmed.
    pub message: String,
    /// First line of the message.
    pub subject: String,
    /// Diff statistics against the first parent (or the empty tree).
    pub stats: DiffStat,
    /// Patch excerpt, capped by `max_patch_bytes`; `None` when disabled (cap 0).
    pub patch: Option<String>,
    /// Whether the patch excerpt was cut at the byte budget.
    pub patch_truncated: bool,
}

/// Status of a single staged file (RFC 0004 R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    /// Newly staged file.
    Added,
    /// Content or mode changed.
    Modified,
    /// Staged for deletion.
    Deleted,
}

/// One staged file with its own patch (RFC 0004 R4).
#[derive(Debug, Clone, Serialize)]
pub struct StagedFile {
    /// Path relative to the repository root.
    pub path: String,
    /// Kind of staged change.
    pub status: FileStatus,
    /// Lines added (0 for binary).
    pub insertions: usize,
    /// Lines removed (0 for binary).
    pub deletions: usize,
    /// Whether either side looks binary (git's NUL-byte heuristic).
    pub binary: bool,
    /// Per-file patch text, capped by `max_file_patch_bytes`; `None` when
    /// disabled (cap 0) or for non-blob entries.
    pub patch: Option<String>,
    /// Whether the patch was cut at the byte budget.
    pub patch_truncated: bool,
}

/// The full staged picture handed to write mode (RFC 0004 R4).
#[derive(Debug, Clone, Serialize)]
pub struct StagedChanges {
    /// Aggregate statistics over all staged files.
    pub stats: DiffStat,
    /// Per-file details, sorted by path.
    pub files: Vec<StagedFile>,
}

/// Truncate `text` to at most `max_bytes` (on a char boundary) and append the
/// truncation marker; returns whether truncation happened.
pub(crate) fn truncate_patch(text: &mut String, max_bytes: usize) -> bool {
    if text.len() <= max_bytes {
        return false;
    }
    let mut cut = max_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text.push_str(TRUNCATION_MARKER);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_patch_respects_char_boundaries() {
        let mut text = "héllo wörld".to_owned();
        let truncated = truncate_patch(&mut text, 2); // inside the 'é'
        assert!(truncated);
        assert!(text.starts_with('h'));
        assert!(text.ends_with(TRUNCATION_MARKER));
    }

    #[test]
    fn truncate_patch_leaves_short_text_alone() {
        let mut text = "short".to_owned();
        assert!(!truncate_patch(&mut text, 100));
        assert_eq!(text, "short");
    }
}
