//! Commit creation — the single write-side Git operation, deliberately
//! delegated to the system `git` binary (RFC 0004 R6) so hooks
//! (`pre-commit`, `commit-msg`), `commit.gpgsign`, and identity resolution
//! behave exactly like a hand-typed `git commit`.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// Result of a successful commit.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    /// Short sha of the new commit.
    pub short_sha: String,
}

/// Commit-creation failures (RFC 0004 R6, RFC 0006 R8).
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// The `git` binary is not installed or not on `PATH` — only commit
    /// creation needs it (RFC 0004 R6).
    #[error(
        "the `git` binary is required to create commits but was not found on PATH; \
         install git or use --dry-run"
    )]
    GitMissing,
    /// git exited non-zero — typically a hook rejection; the output is shown
    /// verbatim and the caller returns to the prompt (RFC 0006 R8).
    #[error("git rejected the commit:\n{output}")]
    Rejected {
        /// Combined stdout+stderr of the failed `git commit`.
        output: String,
    },
    /// Spawning or talking to the subprocess failed.
    #[error("failed to run `git commit`: {0}")]
    Io(#[source] std::io::Error),
}

/// Create a commit with `message` in the repository at `workdir`, feeding the
/// message through stdin (`git commit -F -`). Hooks run and their output is
/// captured for display.
pub fn create_commit(workdir: &Path, message: &str) -> Result<CommitOutcome, CommitError> {
    let mut child = Command::new("git")
        .args(["commit", "-F", "-"])
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CommitError::GitMissing
            } else {
                CommitError::Io(error)
            }
        })?;

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(message.as_bytes())
        .map_err(CommitError::Io)?;
    let output = child.wait_with_output().map_err(CommitError::Io)?;

    if !output.status.success() {
        let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        return Err(CommitError::Rejected {
            output: combined.trim().to_owned(),
        });
    }

    let short_sha = rev_parse_short_head(workdir)?;
    Ok(CommitOutcome { short_sha })
}

/// Resolve the short sha of `HEAD` after a successful commit.
fn rev_parse_short_head(workdir: &Path) -> Result<String, CommitError> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(workdir)
        .output()
        .map_err(CommitError::Io)?;
    if !output.status.success() {
        return Err(CommitError::Rejected {
            output: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
