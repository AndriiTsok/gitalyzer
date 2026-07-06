//! End-to-end write-mode runs (RFC 0006): real binary, fixture repository
//! with staged changes, mock provider — covering the accept/type/regenerate
//! loop (via the internal TTY escape hatch), commit creation through the real
//! `git` with hooks, dry-run, and the JSON envelope.

mod common;

use assert_cmd::Command;
use common::{FIXTURE_EMAIL, FIXTURE_NAME, FixtureRepo};
use predicates::str::contains;
use serde_json::{Value, json};

/// A repo with one base commit and one staged modification.
fn staged_repo() -> FixtureRepo {
    let mut fx = FixtureRepo::new();
    fx.commit_file("chore: base", "src/auth.rs", "fn auth() {}\n");
    fx.write_file("src/auth.rs", "fn auth() {}\nfn validate() {}\n");
    fx.stage(&["src/auth.rs"]);
    fx
}

/// The scripted suggestion(s).
fn script(subjects: &[&str]) -> Value {
    Value::Array(
        subjects
            .iter()
            .map(|subject| {
                json!({
                    "changes_detected": ["Modified authentication logic"],
                    "subject": subject,
                    "body": "- Add validation helper",
                })
            })
            .collect(),
    )
}

/// A gitalyzer write command: isolated env, mock provider, git identity set
/// so the real `git commit` works inside the fixture.
fn gitalyzer(fx: &FixtureRepo, responses: &Value) -> Command {
    let script_path = fx.path().join("mock-script.json");
    std::fs::write(&script_path, responses.to_string()).expect("script written");
    let mut cmd = Command::cargo_bin("gitalyzer").expect("binary builds");
    cmd.current_dir(fx.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH set"))
        .env("HOME", fx.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", FIXTURE_NAME)
        .env("GIT_AUTHOR_EMAIL", FIXTURE_EMAIL)
        .env("GIT_COMMITTER_NAME", FIXTURE_NAME)
        .env("GIT_COMMITTER_EMAIL", FIXTURE_EMAIL)
        .env("GITALYZER_PROVIDER", "mock")
        .env("GITALYZER_MOCK_SCRIPT", &script_path);
    cmd
}

#[test]
fn accepting_with_enter_creates_the_commit_through_git() {
    let fx = staged_repo();
    gitalyzer(&fx, &script(&["refactor(auth): improve error handling"]))
        .arg("write")
        .env("GITALYZER_ASSUME_TTY", "1")
        .write_stdin("\n")
        .assert()
        .success()
        .stdout(contains("Changes detected:"))
        .stdout(contains("- Modified authentication logic"))
        .stdout(contains("Suggested commit message:"))
        .stdout(contains("refactor(auth): improve error handling"))
        .stdout(contains(
            "Press Enter to accept, type your own message, or 'r' to regenerate:",
        ))
        .stdout(contains("✓ Committed"))
        .stderr(contains(
            "Analyzing staged changes... (1 files changed, +1 -0 lines)",
        ));

    assert_eq!(fx.commit_count(), 2, "a new commit must exist");
    assert_eq!(
        fx.head_message(),
        "refactor(auth): improve error handling\n\n- Add validation helper"
    );
}

#[test]
fn typing_a_message_commits_the_typed_text() {
    let fx = staged_repo();
    gitalyzer(&fx, &script(&["ignored suggestion"]))
        .arg("write")
        .env("GITALYZER_ASSUME_TTY", "1")
        .write_stdin("my own subject line\n")
        .assert()
        .success()
        .stdout(contains("✓ Committed"));
    assert_eq!(fx.head_message(), "my own subject line");
}

#[test]
fn regenerate_requests_a_distinct_alternative() {
    let fx = staged_repo();
    gitalyzer(&fx, &script(&["first suggestion", "second suggestion"]))
        .arg("write")
        .env("GITALYZER_ASSUME_TTY", "1")
        .write_stdin("r\n\n")
        .assert()
        .success()
        .stdout(contains("first suggestion"))
        .stdout(contains("second suggestion"))
        .stderr(contains("Regenerating..."));
    assert!(
        fx.head_message().starts_with("second suggestion"),
        "second wins"
    );
}

#[test]
fn hook_rejection_keeps_staged_state_and_reprompts() {
    let fx = staged_repo();
    // A commit-msg hook that rejects any message containing "bad".
    fx.install_hook(
        "commit-msg",
        "#!/bin/sh\nif grep -q bad \"$1\"; then echo 'rejected: message is bad' >&2; exit 1; fi\n",
    );
    gitalyzer(&fx, &script(&["ignored"]))
        .arg("write")
        .env("GITALYZER_ASSUME_TTY", "1")
        .write_stdin("bad message\ngood message\n")
        .assert()
        .success()
        .stdout(contains("✓ Committed"))
        .stderr(contains("rejected: message is bad"))
        .stderr(contains("The commit was rejected"));

    assert_eq!(fx.head_message(), "good message");
    assert_eq!(fx.commit_count(), 2, "exactly one commit landed");
}

#[test]
fn dry_run_prints_the_message_and_never_commits() {
    let fx = staged_repo();
    // Piped stdin without the TTY escape hatch → non-interactive dry run.
    gitalyzer(&fx, &script(&["feat(auth): add validation"]))
        .args(["write", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("feat(auth): add validation"))
        .stdout(contains("- Add validation helper"));
    assert_eq!(fx.commit_count(), 1, "dry run must not commit");
}

#[test]
fn json_mode_is_non_interactive_and_never_commits() {
    let fx = staged_repo();
    let output = gitalyzer(&fx, &script(&["feat(auth): add validation"]))
        .args(["write", "--format", "json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // RFC 0007 R2: stdout is exactly one JSON document, stderr silent.
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("clean JSON stdout");
    assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), "");

    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["mode"], "write");
    assert_eq!(envelope["staged"]["files_changed"], 1);
    assert_eq!(envelope["staged"]["files"][0], "src/auth.rs");
    assert_eq!(
        envelope["suggestion"]["subject"],
        "feat(auth): add validation"
    );
    assert_eq!(envelope["suggestion"]["style"], "auto");
    assert_eq!(envelope["meta"]["provider"], "mock");
    assert_eq!(fx.commit_count(), 1, "json mode must not commit");
}

#[test]
fn interactive_mode_without_a_tty_fails_actionably() {
    let fx = staged_repo();
    gitalyzer(&fx, &script(&["x"]))
        .arg("write")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("--dry-run"))
        .stderr(contains("--format json"));
    assert_eq!(fx.commit_count(), 1);
}

#[test]
fn nothing_staged_is_an_actionable_error() {
    let mut fx = FixtureRepo::new();
    fx.commit_file("chore: base", "a.txt", "x\n");
    gitalyzer(&fx, &script(&["x"]))
        .arg("write")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("nothing is staged"));
}
