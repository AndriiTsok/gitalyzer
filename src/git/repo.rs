//! Repository discovery and history extraction (RFC 0004 R1–R3).

use std::path::Path;

use gix::bstr::ByteSlice;
use gix::diff::tree_with_rewrites::Change;
use gix::prelude::ObjectIdExt as _;

use super::blobdiff::diff_blobs;
use super::{CommitInfo, DiffStat, GitError, internal, truncate_patch};

/// How much history to extract and how big patch excerpts may grow.
#[derive(Debug, Clone)]
pub struct HistoryOptions {
    /// Start revision; `None` means `HEAD` (RFC 0001 R4 `--from`).
    pub from: Option<String>,
    /// Number of non-merge commits to collect (RFC 0004 R2).
    pub count: usize,
    /// Per-commit patch excerpt cap in bytes; `0` disables patch content
    /// entirely (RFC 0005 R3).
    pub max_patch_bytes: u64,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            from: None,
            count: 50,
            max_patch_bytes: 4096,
        }
    }
}

/// A discovered repository; all reads go through gitoxide (RFC 0004 R1).
pub struct Repo {
    inner: gix::Repository,
}

impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repo")
            .field("git_dir", &self.inner.git_dir())
            .finish()
    }
}

impl Repo {
    /// Discover the repository containing the current working directory.
    pub fn discover() -> Result<Self, GitError> {
        let cwd = std::env::current_dir().map_err(internal)?;
        Self::discover_at(&cwd)
    }

    /// Discover the repository containing `dir` (upward search).
    pub fn discover_at(dir: &Path) -> Result<Self, GitError> {
        let inner = gix::discover(dir).map_err(|_| GitError::NotARepository {
            path: dir.display().to_string(),
        })?;
        Ok(Self { inner })
    }

    /// Access the underlying gitoxide repository (used by sibling modules).
    pub(crate) fn inner(&self) -> &gix::Repository {
        &self.inner
    }

    /// The working tree root; `None` for bare repositories.
    pub fn workdir(&self) -> Option<&Path> {
        self.inner.workdir()
    }

    /// Subjects of up to `limit` recent non-merge commits, newest first —
    /// the style-inference context of RFC 0006 R4. An empty or unborn
    /// history yields an empty list (style then falls back to Conventional
    /// Commits), never an error.
    pub fn recent_subjects(&self, limit: usize) -> Result<Vec<String>, GitError> {
        let Ok(start) = self.inner.rev_parse_single("HEAD") else {
            return Ok(Vec::new());
        };
        let walk = self
            .inner
            .rev_walk(Some(start.detach()))
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(internal)?;

        let mut subjects = Vec::new();
        for info in walk {
            if subjects.len() >= limit {
                break;
            }
            let info = info.map_err(internal)?;
            if info.parent_ids.len() > 1 {
                continue;
            }
            let commit = self.inner.find_commit(info.id).map_err(internal)?;
            let message = commit.message().map_err(internal)?;
            subjects.push(message.title.to_str_lossy().trim().to_owned());
        }
        Ok(subjects)
    }

    /// Walk history newest-first from `--from` (default `HEAD`), skipping
    /// merge commits, and extract per-commit context (RFC 0004 R1–R3).
    pub fn history(&self, options: &HistoryOptions) -> Result<Vec<CommitInfo>, GitError> {
        let spec = options.from.as_deref().unwrap_or("HEAD");
        let start = self.inner.rev_parse_single(spec).map_err(|_| {
            if options.from.is_none() {
                // An unresolvable HEAD means an unborn branch: no commits yet.
                GitError::NoCommits
            } else {
                GitError::BadRevision {
                    spec: spec.to_owned(),
                }
            }
        })?;

        let walk = self
            .inner
            .rev_walk(Some(start.detach()))
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(internal)?;

        let mut commits = Vec::new();
        for info in walk {
            if commits.len() >= options.count {
                break;
            }
            let info = info.map_err(internal)?;
            // RFC 0004 R2: merge commits are skipped; the walk continues so
            // the requested count is still filled from regular commits.
            if info.parent_ids.len() > 1 {
                continue;
            }
            commits.push(self.extract(&info, options.max_patch_bytes)?);
        }

        if commits.is_empty() {
            return Err(GitError::NoCommits);
        }
        Ok(commits)
    }

    /// Extract metadata, diffstat, and a capped patch for one commit.
    fn extract(
        &self,
        info: &gix::revision::walk::Info<'_>,
        max_patch_bytes: u64,
    ) -> Result<CommitInfo, GitError> {
        let commit = self.inner.find_commit(info.id).map_err(internal)?;

        let message_ref = commit.message().map_err(internal)?;
        let subject = message_ref.title.to_str_lossy().trim().to_owned();
        let message = commit.message_raw_sloppy().to_str_lossy().trim().to_owned();

        let author = commit.author().map_err(internal)?;
        let date = author.time().map_or_else(
            |_| author.time.to_owned(),
            |time| time.format_or_unix(gix::date::time::format::ISO8601_STRICT),
        );

        let sha = info.id.to_string();
        let short_sha = info.id.attach(&self.inner).shorten_or_id().to_string();

        let tree = commit.tree().map_err(internal)?;
        let parent_tree = match info.parent_ids.first() {
            Some(parent_id) => Some(
                self.inner
                    .find_commit(*parent_id)
                    .map_err(internal)?
                    .tree()
                    .map_err(internal)?,
            ),
            // Root commits diff against the empty tree (RFC 0004 R3).
            None => None,
        };

        let changes = self
            .inner
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
            .map_err(internal)?;

        let (stats, patch, patch_truncated) = self.digest_changes(&changes, max_patch_bytes)?;

        Ok(CommitInfo {
            sha,
            short_sha,
            author_name: author.name.to_str_lossy().into_owned(),
            author_email: author.email.to_str_lossy().into_owned(),
            date,
            message,
            subject,
            stats,
            patch,
            patch_truncated,
        })
    }

    /// Fold tree changes into aggregate stats plus one capped patch excerpt.
    fn digest_changes(
        &self,
        changes: &[Change],
        max_patch_bytes: u64,
    ) -> Result<(DiffStat, Option<String>, bool), GitError> {
        let budget = usize::try_from(max_patch_bytes).unwrap_or(usize::MAX);
        let want_patch = budget > 0;
        let mut stats = DiffStat::default();
        let mut patch = String::new();
        let mut truncated = false;

        for change in changes {
            let Some(file) = FileChange::classify(change) else {
                continue;
            };
            stats.files_changed += 1;
            stats.files.push(file.location.clone());

            if !file.diffable {
                continue;
            }
            let old = self.blob_data(file.old_id)?;
            let new = self.blob_data(file.new_id)?;
            // Text is only rendered while the budget has room; counts always.
            let render = want_patch && !truncated;
            let diff = diff_blobs(&old, &new, render);
            stats.insertions += diff.insertions;
            stats.deletions += diff.deletions;

            if let Some(text) = diff.text {
                patch.push_str(&file.header);
                patch.push_str(&text);
                if truncate_patch(&mut patch, budget) {
                    truncated = true;
                }
            }
        }

        let patch = (want_patch && !patch.is_empty()).then_some(patch);
        Ok((stats, patch, truncated))
    }

    /// Load blob bytes, treating `None` (absent side) as empty content.
    fn blob_data(&self, id: Option<gix::ObjectId>) -> Result<Vec<u8>, GitError> {
        match id {
            Some(id) if !id.is_empty_blob() => {
                Ok(self.inner.find_object(id).map_err(internal)?.data.clone())
            }
            _ => Ok(Vec::new()),
        }
    }
}

/// A tree change reduced to what extraction needs.
struct FileChange {
    location: String,
    header: String,
    old_id: Option<gix::ObjectId>,
    new_id: Option<gix::ObjectId>,
    /// Whether blob content should be diffed (false for submodules etc.).
    diffable: bool,
}

impl FileChange {
    /// Reduce a tree change to paths/ids; `None` for tree-level entries.
    fn classify(change: &Change) -> Option<Self> {
        use gix::object::tree::EntryKind;

        let is_content = |mode: gix::object::tree::EntryMode| {
            matches!(
                mode.kind(),
                EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link
            )
        };

        match change {
            Change::Addition {
                location,
                entry_mode,
                id,
                ..
            } => {
                if entry_mode.is_tree() {
                    return None;
                }
                let path = location.to_str_lossy().into_owned();
                Some(Self {
                    header: format!("--- /dev/null\n+++ b/{path}\n"),
                    location: path,
                    old_id: None,
                    new_id: Some(*id),
                    diffable: is_content(*entry_mode),
                })
            }
            Change::Deletion {
                location,
                entry_mode,
                id,
                ..
            } => {
                if entry_mode.is_tree() {
                    return None;
                }
                let path = location.to_str_lossy().into_owned();
                Some(Self {
                    header: format!("--- a/{path}\n+++ /dev/null\n"),
                    location: path,
                    old_id: Some(*id),
                    new_id: None,
                    diffable: is_content(*entry_mode),
                })
            }
            Change::Modification {
                location,
                previous_entry_mode,
                previous_id,
                entry_mode,
                id,
            } => {
                if entry_mode.is_tree() {
                    return None;
                }
                let path = location.to_str_lossy().into_owned();
                Some(Self {
                    header: format!("--- a/{path}\n+++ b/{path}\n"),
                    location: path,
                    old_id: Some(*previous_id),
                    new_id: Some(*id),
                    diffable: is_content(*previous_entry_mode) || is_content(*entry_mode),
                })
            }
            Change::Rewrite {
                source_location,
                location,
                source_id,
                id,
                entry_mode,
                ..
            } => {
                if entry_mode.is_tree() {
                    return None;
                }
                let from = source_location.to_str_lossy();
                let path = location.to_str_lossy().into_owned();
                Some(Self {
                    header: format!("--- a/{from}\n+++ b/{path}\n"),
                    location: format!("{from} -> {path}"),
                    old_id: Some(*source_id),
                    new_id: Some(*id),
                    diffable: is_content(*entry_mode),
                })
            }
        }
    }
}
