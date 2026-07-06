//! Remote analysis support (RFC 0004 R5, R9): bare shallow clones of any Git
//! transport into self-cleaning temporary directories, authenticating exactly
//! as the user's own `git` would — gitoxide loads the git installation's
//! configuration, so SSH config/agent and credential helpers apply.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};

use tempfile::TempDir;

use super::{GitError, Repo};

/// Extra depth on top of the requested commit count, absorbing a few skipped
/// merge commits before a full re-clone becomes necessary (RFC 0004 R5).
const DEPTH_BUFFER: u32 = 5;

/// Process-wide interrupt flag observed by in-flight clones; set by the
/// Ctrl-C handler so fetches abort quickly and temp dirs get dropped
/// (RFC 0004 R5, RFC 0007 R9).
static SHOULD_INTERRUPT: AtomicBool = AtomicBool::new(false);

/// Request that any in-flight clone abort at the next opportunity.
pub fn interrupt_clones() {
    SHOULD_INTERRUPT.store(true, Ordering::Relaxed);
}

/// A cloned remote repository; the backing temporary directory lives exactly
/// as long as this value (RFC 0004 R5, R7).
pub struct RemoteClone {
    /// The opened bare clone.
    pub repo: Repo,
    /// Whether the clone ended up with a shallow boundary.
    pub shallow: bool,
    _tempdir: TempDir,
}

impl std::fmt::Debug for RemoteClone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteClone")
            .field("shallow", &self.shallow)
            .finish_non_exhaustive()
    }
}

/// Bare-clone `url` for analysis (RFC 0004 R5).
///
/// `depth` bounds history transfer (`None` = full clone — used when `--from`
/// must resolve arbitrary history, and by the automatic re-clone when a
/// shallow boundary proved too tight). `branch` selects what `HEAD` points
/// at; `None` uses the remote's default branch.
pub fn clone_for_analysis(
    url: &str,
    branch: Option<&str>,
    depth: Option<u32>,
) -> Result<RemoteClone, GitError> {
    let tempdir = TempDir::with_prefix("gitalyzer-clone-").map_err(super::internal)?;

    let mut prepare = gix::prepare_clone_bare(url, tempdir.path())
        .map_err(|error| classify(url, &error_chain(&error)))?;
    if let Some(name) = branch {
        prepare = prepare
            .with_ref_name(Some(name))
            .map_err(|error: gix::refs::name::Error| GitError::CloneFailed {
                url: url.to_owned(),
                message: format!("`{name}` is not a valid ref name: {error}"),
            })?;
    }
    if let Some(depth) = depth {
        let depth = NonZeroU32::new(depth.saturating_add(DEPTH_BUFFER).max(1))
            .expect("max(1) guarantees non-zero");
        prepare = prepare.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(depth));
    }

    let (repository, _outcome) = prepare
        .fetch_only(gix::progress::Discard, &SHOULD_INTERRUPT)
        .map_err(|error| classify(url, &error_chain(&error)))?;

    let shallow = repository.is_shallow();
    Ok(RemoteClone {
        repo: Repo::from_gix(repository),
        shallow,
        _tempdir: tempdir,
    })
}

/// Flatten an error and all its sources into one line — gix nests the
/// telling details (auth failures especially) several levels deep.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    parts.join(": ")
}

/// Map a clone failure to an actionable error (RFC 0004 R8–R9): credential
/// problems point at the user's own git auth setup, since that is exactly
/// what Gitalyzer uses.
fn classify(url: &str, message: &str) -> GitError {
    let lowered = message.to_lowercase();
    let auth = [
        "authentication",
        "credential",
        "permission denied",
        "401",
        "403",
        "publickey",
    ]
    .iter()
    .any(|needle| lowered.contains(needle));
    if auth {
        GitError::CloneAuthFailed {
            url: url.to_owned(),
            message: message.to_owned(),
        }
    } else {
        GitError::CloneFailed {
            url: url.to_owned(),
            message: message.to_owned(),
        }
    }
}
