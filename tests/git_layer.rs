//! Integration tests for the git layer (RFC 0004 R1–R4) against scripted
//! fixture repositories.

mod common;

use common::{FIXTURE_EMAIL, FIXTURE_NAME, FixtureRepo};
use gitalyzer::git::{
    FileStatus, GitError, HistoryOptions, Repo, TRUNCATION_MARKER, staged_changes,
};

/// Convenience: history with explicit knobs.
fn history(
    repo: &FixtureRepo,
    from: Option<&str>,
    count: usize,
    max_patch: u64,
) -> Vec<gitalyzer::git::CommitInfo> {
    Repo::discover_at(repo.path())
        .expect("fixture is a repository")
        .history(&HistoryOptions {
            from: from.map(str::to_owned),
            count,
            max_patch_bytes: max_patch,
        })
        .expect("history extraction succeeds")
}

#[test]
fn history_is_newest_first_with_full_metadata() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("first commit", "a.txt", "one\n");
    fx.commit_file("second commit", "a.txt", "one\ntwo\n");
    let third = fx.commit_file("third commit", "b.txt", "hello\n");

    let commits = history(&fx, None, 50, 4096);

    assert_eq!(commits.len(), 3);
    let subjects: Vec<_> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["third commit", "second commit", "first commit"]);

    let newest = &commits[0];
    assert_eq!(newest.sha, third);
    assert!(newest.sha.len() >= 40, "full sha expected");
    assert!(newest.sha.starts_with(&newest.short_sha));
    assert_eq!(newest.author_name, FIXTURE_NAME);
    assert_eq!(newest.author_email, FIXTURE_EMAIL);
    assert!(
        newest.date.starts_with("20"),
        "ISO date, got: {}",
        newest.date
    );
    assert!(newest.date.contains('T'), "ISO date, got: {}", newest.date);
    assert_eq!(newest.message, "third commit");
}

#[test]
fn history_respects_count_and_from() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("c1", "f.txt", "1\n");
    let second = fx.commit_file("c2", "f.txt", "1\n2\n");
    fx.commit_file("c3", "f.txt", "1\n2\n3\n");

    let limited = history(&fx, None, 2, 0);
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].subject, "c3");

    let from_second = history(&fx, Some(&second), 50, 0);
    let subjects: Vec<_> = from_second.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["c2", "c1"]);
}

#[test]
fn merge_commits_are_skipped_and_backfilled() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("base", "a.txt", "base\n");
    fx.branch("side");
    fx.commit_file("side work", "side.txt", "side\n");
    fx.checkout("main");
    fx.commit_file("main work", "main.txt", "main\n");
    fx.merge("side", "Merge branch 'side'");

    let commits = history(&fx, None, 50, 0);
    let subjects: Vec<_> = commits.iter().map(|c| c.subject.as_str()).collect();

    assert!(
        !subjects.iter().any(|s| s.starts_with("Merge")),
        "merge must be skipped: {subjects:?}"
    );
    assert_eq!(subjects, ["main work", "side work", "base"]);
}

#[test]
fn diffstat_counts_files_and_lines() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("base", "a.txt", "one\ntwo\nthree\n");
    fx.write_file("a.txt", "one\nTWO\nthree\n"); // 1 del + 1 ins
    fx.write_file("b.txt", "brand new\nfile\n"); // 2 ins
    fx.stage(&["a.txt", "b.txt"]);
    fx.commit("touch two files");

    let commits = history(&fx, None, 1, 4096);
    let stats = &commits[0].stats;

    assert_eq!(stats.files_changed, 2);
    assert_eq!(stats.insertions, 3);
    assert_eq!(stats.deletions, 1);
    assert!(stats.files.contains(&"a.txt".to_owned()));
    assert!(stats.files.contains(&"b.txt".to_owned()));
}

#[test]
fn root_commit_diffs_against_the_empty_tree() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("root", "a.txt", "one\ntwo\n");

    let commits = history(&fx, None, 1, 4096);
    let root = &commits[0];

    assert_eq!(root.stats.files_changed, 1);
    assert_eq!(root.stats.insertions, 2);
    assert_eq!(root.stats.deletions, 0);
    let patch = root.patch.as_deref().expect("patch requested");
    assert!(patch.contains("+++ b/a.txt"), "got: {patch}");
    assert!(patch.contains("+one"), "got: {patch}");
}

#[test]
fn patch_is_capped_with_marker_and_can_be_disabled() {
    let mut fx = FixtureRepo::new();
    let big: String = (0..200).fold(String::new(), |mut s, i| {
        use std::fmt::Write as _;
        let _ = writeln!(s, "line number {i}");
        s
    });
    fx.commit_file("big file", "big.txt", &big);

    let capped = history(&fx, None, 1, 64);
    assert!(capped[0].patch_truncated);
    let patch = capped[0].patch.as_deref().expect("patch requested");
    assert!(patch.ends_with(TRUNCATION_MARKER), "got: {patch}");
    assert!(patch.len() <= 64 + TRUNCATION_MARKER.len());
    // Stats are unaffected by the text budget.
    assert_eq!(capped[0].stats.insertions, 200);

    let disabled = history(&fx, None, 1, 0);
    assert!(disabled[0].patch.is_none());
    assert!(!disabled[0].patch_truncated);
    assert_eq!(disabled[0].stats.insertions, 200);
}

#[test]
fn unresolvable_from_is_an_actionable_error() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("only", "a.txt", "x\n");

    let error = Repo::discover_at(fx.path())
        .expect("repo")
        .history(&HistoryOptions {
            from: Some("no-such-ref".into()),
            count: 5,
            max_patch_bytes: 0,
        })
        .expect_err("bad revision must fail");

    assert!(
        matches!(error, GitError::BadRevision { .. }),
        "got: {error:?}"
    );
    assert!(error.to_string().contains("no-such-ref"));
}

#[test]
fn empty_repository_history_is_an_error() {
    let fx = FixtureRepo::new();
    let error = Repo::discover_at(fx.path())
        .expect("repo")
        .history(&HistoryOptions::default())
        .expect_err("no commits must fail");
    assert!(matches!(error, GitError::NoCommits), "got: {error:?}");
}

#[test]
fn discovery_outside_a_repository_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = Repo::discover_at(dir.path()).expect_err("not a repo");
    assert!(
        matches!(error, GitError::NotARepository { .. }),
        "got: {error:?}"
    );
}

#[test]
fn staged_changes_cover_add_modify_delete() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("base a", "a.txt", "one\ntwo\n");
    fx.commit_file("base c", "c.txt", "gone soon\n");

    fx.write_file("a.txt", "one\ntwo\nthree\n");
    fx.write_file("b.txt", "fresh\n");
    fx.stage(&["a.txt", "b.txt"]);
    fx.stage_removal("c.txt");

    let repo = Repo::discover_at(fx.path()).expect("repo");
    let staged = staged_changes(&repo, 4096).expect("staged extraction");

    assert_eq!(staged.stats.files_changed, 3);
    assert_eq!(staged.stats.insertions, 2); // +three, +fresh
    assert_eq!(staged.stats.deletions, 1); // -gone soon
    assert_eq!(staged.stats.files, ["a.txt", "b.txt", "c.txt"]);

    let by_path = |p: &str| {
        staged
            .files
            .iter()
            .find(|f| f.path == p)
            .expect("file present")
    };
    assert_eq!(by_path("a.txt").status, FileStatus::Modified);
    assert_eq!(by_path("b.txt").status, FileStatus::Added);
    assert_eq!(by_path("c.txt").status, FileStatus::Deleted);
    assert!(
        by_path("a.txt")
            .patch
            .as_deref()
            .expect("patch")
            .contains("+three")
    );
    assert!(
        by_path("c.txt")
            .patch
            .as_deref()
            .expect("patch")
            .contains("-gone soon")
    );
}

#[test]
fn unstaged_worktree_edits_are_invisible() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("base", "a.txt", "one\n");
    fx.write_file("a.txt", "edited but never staged\n");

    let repo = Repo::discover_at(fx.path()).expect("repo");
    let error = staged_changes(&repo, 0).expect_err("nothing staged");
    assert!(matches!(error, GitError::NothingStaged), "got: {error:?}");
}

#[test]
fn staged_changes_work_on_an_unborn_branch() {
    let fx = FixtureRepo::new();
    fx.write_file("first.txt", "hello\n");
    fx.stage(&["first.txt"]);

    let repo = Repo::discover_at(fx.path()).expect("repo");
    let staged = staged_changes(&repo, 4096).expect("unborn HEAD must work");

    assert_eq!(staged.stats.files_changed, 1);
    assert_eq!(staged.files[0].status, FileStatus::Added);
    assert_eq!(staged.files[0].insertions, 1);
}

#[test]
fn binary_staged_files_are_flagged_without_line_noise() {
    let fx = FixtureRepo::new();
    fx.write_file("blob.bin", [0u8, 159, 146, 150, 0, 1, 2]);
    fx.stage(&["blob.bin"]);

    let repo = Repo::discover_at(fx.path()).expect("repo");
    let staged = staged_changes(&repo, 4096).expect("staged extraction");

    let file = &staged.files[0];
    assert!(file.binary);
    assert_eq!(file.insertions, 0);
    assert_eq!(file.deletions, 0);
    assert_eq!(file.patch.as_deref(), Some("Binary files differ\n"));
}
