//! Shared test infrastructure: scripted Git fixture repositories (RFC 0007
//! R11). Fixtures are built with the system `git` binary — test-only tooling,
//! not product code — fully isolated from host configuration, with
//! deterministic identities and monotonically increasing commit dates so
//! time-ordered walks are stable.
#![allow(dead_code)] // shared across independent test binaries; not all use every helper

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Deterministic author/committer identity for all fixture commits.
pub const FIXTURE_NAME: &str = "Test Author";
/// Deterministic email for all fixture commits.
pub const FIXTURE_EMAIL: &str = "test@example.com";

/// A throwaway Git repository scripted through the `git` binary.
pub struct FixtureRepo {
    dir: TempDir,
    ticks: u64,
}

impl FixtureRepo {
    /// Initialize an empty repository on branch `main`.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("fixture tempdir");
        let repo = Self { dir, ticks: 0 };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    /// Root path of the working tree.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run a git subcommand, isolated from host config, and return stdout.
    pub fn git(&self, args: &[&str]) -> String {
        let date = format!("@{} +0000", 1_750_000_000 + self.ticks * 60);
        let output = Command::new("git")
            .args(args)
            .current_dir(self.dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", FIXTURE_NAME)
            .env("GIT_AUTHOR_EMAIL", FIXTURE_EMAIL)
            .env("GIT_COMMITTER_NAME", FIXTURE_NAME)
            .env("GIT_COMMITTER_EMAIL", FIXTURE_EMAIL)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .expect("git binary available for fixtures");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Write (or overwrite) a file relative to the repository root.
    pub fn write_file(&self, rel: &str, content: impl AsRef<[u8]>) {
        let path = self.dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture dirs");
        }
        std::fs::write(path, content).expect("fixture file");
    }

    /// Stage the given paths.
    pub fn stage(&self, paths: &[&str]) {
        let mut args = vec!["add", "--"];
        args.extend_from_slice(paths);
        self.git(&args);
    }

    /// Commit whatever is staged; returns the commit SHA. Each commit gets a
    /// strictly newer timestamp than the previous one.
    pub fn commit(&mut self, message: &str) -> String {
        self.ticks += 1;
        self.git(&["commit", "-q", "--no-verify", "-m", message]);
        self.head()
    }

    /// Write + stage + commit one file in a single step; returns the SHA.
    pub fn commit_file(&mut self, message: &str, rel: &str, content: &str) -> String {
        self.write_file(rel, content);
        self.stage(&[rel]);
        self.commit(message)
    }

    /// Create and switch to a new branch.
    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "-q", "-b", name]);
    }

    /// Switch to an existing branch.
    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    /// Merge `branch` with a merge commit; returns the merge SHA.
    pub fn merge(&mut self, branch: &str, message: &str) -> String {
        self.ticks += 1;
        self.git(&[
            "merge",
            "--no-ff",
            "-q",
            "--no-verify",
            "-m",
            message,
            branch,
        ]);
        self.head()
    }

    /// Stage a file deletion.
    pub fn stage_removal(&self, rel: &str) {
        self.git(&["rm", "-q", "--", rel]);
    }

    /// Current `HEAD` SHA.
    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }
}
