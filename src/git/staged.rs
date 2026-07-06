//! Staged-change extraction: HEAD tree vs index (RFC 0004 R4).
//!
//! The comparison deliberately maps both sides to index form (the HEAD tree
//! via [`gix::Repository::index_from_tree`]) and diffs entry-by-entry: staged
//! blobs already live in the object database after `git add`, so both sides
//! resolve without touching the worktree. Worktree edits that were never
//! staged are — correctly — invisible here.

use std::collections::BTreeMap;

use gix::bstr::{BString, ByteSlice};

use super::blobdiff::diff_blobs;
use super::{
    DiffStat, FileStatus, GitError, Repo, StagedChanges, StagedFile, internal, truncate_patch,
};

/// One side of the comparison: blob id + entry mode.
type EntryMap = BTreeMap<BString, (gix::ObjectId, gix::index::entry::Mode)>;

/// Extract the staged changes of `repo`.
///
/// `max_file_patch_bytes` caps each file's patch text (`0` disables patch
/// content; budgeting policy on top is RFC 0006 R3 business logic). An empty
/// result is the actionable [`GitError::NothingStaged`].
pub fn staged_changes(repo: &Repo, max_file_patch_bytes: u64) -> Result<StagedChanges, GitError> {
    let inner = repo.inner();

    // Unborn HEAD (no commits yet) yields the empty tree: everything staged
    // is then an addition — `write` must work in brand-new repositories.
    let head_tree_id = inner.head_tree_id_or_empty().map_err(internal)?;
    let head_index = inner.index_from_tree(&head_tree_id).map_err(internal)?;
    let index = inner.index_or_empty().map_err(internal)?;

    let old = entry_map(head_index.entries(), &head_index);
    let new = entry_map(index.entries(), &index);

    let budget = usize::try_from(max_file_patch_bytes).unwrap_or(usize::MAX);
    let mut files = Vec::new();
    let mut stats = DiffStat::default();

    // Sorted union of both sides; each path is processed exactly once.
    let all_paths: std::collections::BTreeSet<&BString> = old.keys().chain(new.keys()).collect();
    for path in all_paths {
        let before = old.get(path);
        let after = new.get(path);
        let (kind, old_entry, new_entry) = match (before, after) {
            (None, Some(entry)) => (FileStatus::Added, None, Some(entry)),
            (Some(entry), None) => (FileStatus::Deleted, Some(entry), None),
            (Some(old_entry), Some(new_entry)) => {
                if old_entry == new_entry {
                    continue; // unchanged
                }
                (FileStatus::Modified, Some(old_entry), Some(new_entry))
            }
            (None, None) => unreachable!("path came from one of the maps"),
        };

        let file = diff_entry(repo, path, kind, old_entry, new_entry, budget)?;
        stats.files_changed += 1;
        stats.insertions += file.insertions;
        stats.deletions += file.deletions;
        stats.files.push(file.path.clone());
        files.push(file);
    }

    if files.is_empty() {
        return Err(GitError::NothingStaged);
    }
    Ok(StagedChanges { stats, files })
}

/// Collect stage-0 entries of an index state into a sorted map.
fn entry_map(entries: &[gix::index::Entry], state: &gix::index::State) -> EntryMap {
    entries
        .iter()
        .filter(|entry| entry.stage() == gix::index::entry::Stage::Unconflicted)
        .map(|entry| (entry.path(state).to_owned(), (entry.id, entry.mode)))
        .collect()
}

/// Diff one staged path into a [`StagedFile`].
fn diff_entry(
    repo: &Repo,
    path: &BString,
    kind: FileStatus,
    old_entry: Option<&(gix::ObjectId, gix::index::entry::Mode)>,
    new_entry: Option<&(gix::ObjectId, gix::index::entry::Mode)>,
    budget: usize,
) -> Result<StagedFile, GitError> {
    let is_content = |mode: gix::index::entry::Mode| {
        matches!(
            mode,
            gix::index::entry::Mode::FILE
                | gix::index::entry::Mode::FILE_EXECUTABLE
                | gix::index::entry::Mode::SYMLINK
        )
    };
    let diffable = old_entry.is_some_and(|(_, mode)| is_content(*mode))
        || new_entry.is_some_and(|(_, mode)| is_content(*mode));

    let mut insertions = 0;
    let mut deletions = 0;
    let mut binary = false;
    let mut patch_text = None;
    let mut patch_truncated = false;

    if diffable {
        let old_data = blob_data(repo, old_entry.map(|(id, _)| *id))?;
        let new_data = blob_data(repo, new_entry.map(|(id, _)| *id))?;
        let diff = diff_blobs(&old_data, &new_data, budget > 0);
        insertions = diff.insertions;
        deletions = diff.deletions;
        binary = diff.binary;
        patch_text = diff.text.map(|mut text| {
            patch_truncated = truncate_patch(&mut text, budget);
            text
        });
    }

    Ok(StagedFile {
        path: path.to_str_lossy().into_owned(),
        status: kind,
        insertions,
        deletions,
        binary,
        patch: patch_text,
        patch_truncated,
    })
}

/// Load blob bytes for an optional id; absent sides are empty content.
fn blob_data(repo: &Repo, id: Option<gix::ObjectId>) -> Result<Vec<u8>, GitError> {
    match id {
        Some(id) if !id.is_empty_blob() => {
            Ok(repo.inner().find_object(id).map_err(internal)?.data.clone())
        }
        _ => Ok(Vec::new()),
    }
}
