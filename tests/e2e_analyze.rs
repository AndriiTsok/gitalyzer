//! End-to-end analysis runs: real binary, scripted fixture repository, mock
//! provider selected via configuration (RFC 0007 R11) — asserting the human
//! report shape, the clean-JSON guarantee, batching, and `--output`.

mod common;

use assert_cmd::Command;
use common::FixtureRepo;
use predicates::str::contains;
use serde_json::{Value, json};

/// Fixture with three commits: two weak, one strong. Returns their short
/// (7-char) sha prefixes, oldest first.
fn seeded_repo() -> (FixtureRepo, Vec<String>) {
    let mut fx = FixtureRepo::new();
    let shas = [
        fx.commit_file("wip", "a.txt", "one\n"),
        fx.commit_file("fix stuff", "a.txt", "one\ntwo\n"),
        fx.commit_file(
            "feat(api): add caching layer\n\n- cache read endpoints\n- improves latency",
            "b.txt",
            "cache\n",
        ),
    ];
    let short = shas.iter().map(|s| s[..7].to_owned()).collect();
    (fx, short)
}

/// Critique JSON for the three seeded commits (newest first in walk order,
/// but matching is sha-based so order does not matter).
fn critiques(short: &[String]) -> Value {
    json!([{ "critiques": [
        { "sha": short[0], "score": 1, "issue": "No information about what's in progress",
          "better": "Describe what you're working on",
          "tags": { "vague": true, "misleading": false, "no_why": true } },
        { "sha": short[1], "score": 4, "issue": "Which fix? What stuff?",
          "better": "fix(parser): handle empty input",
          "tags": { "vague": true, "misleading": false, "no_why": true } },
        { "sha": short[2], "score": 9, "why_good": "Clear scope, specific changes",
          "tags": { "vague": false, "misleading": false, "no_why": false } },
    ]}])
}

/// A gitalyzer command inside the fixture repo, isolated, mock provider on.
fn gitalyzer(fx: &FixtureRepo, script: &Value) -> Command {
    let script_path = fx.path().join("mock-script.json");
    std::fs::write(&script_path, script.to_string()).expect("script written");
    let mut cmd = Command::cargo_bin("gitalyzer").expect("binary builds");
    cmd.current_dir(fx.path())
        .env_clear()
        .env("HOME", fx.path())
        .env("GITALYZER_PROVIDER", "mock")
        .env("GITALYZER_MOCK_SCRIPT", &script_path);
    cmd
}

#[test]
fn human_report_has_the_prd_sections_and_progress_on_stderr() {
    let (fx, short) = seeded_repo();
    gitalyzer(&fx, &critiques(&short))
        .arg("analyze")
        .assert()
        .success()
        .stdout(contains("💩 COMMITS THAT NEED WORK"))
        .stdout(contains("Commit: \"wip\""))
        .stdout(contains("Score: 1/10"))
        .stdout(contains("Better: fix(parser): handle empty input"))
        .stdout(contains("💎 WELL-WRITTEN COMMITS"))
        .stdout(contains("Why it's good: Clear scope, specific changes"))
        .stdout(contains("📊 YOUR STATS"))
        .stdout(contains("Average score: 4.7/10"))
        .stdout(contains("Vague commits: 2 (67%)"))
        .stdout(contains("One-word commits: 1 (33%)"))
        .stderr(contains("Analyzing last 50 commits..."));
}

#[test]
fn json_mode_emits_exactly_one_clean_document() {
    let (fx, short) = seeded_repo();
    let output = gitalyzer(&fx, &critiques(&short))
        .args(["analyze", "--format", "json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // RFC 0007 R2: the WHOLE stdout must parse as one JSON document.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let envelope: Value = serde_json::from_str(&stdout).expect("clean JSON stdout");

    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["mode"], "analyze");
    assert_eq!(envelope["repository"]["source"], "local");
    assert_eq!(envelope["range"]["requested"], 50);
    assert_eq!(envelope["range"]["analyzed"], 3);
    assert_eq!(envelope["commits"].as_array().expect("commits").len(), 3);
    assert_eq!(envelope["commits"][0]["score"], 9, "newest first");
    assert_eq!(
        envelope["commits"][2]["one_word"], true,
        "`wip` is one word"
    );
    assert_eq!(envelope["stats"]["vague"]["count"], 2);
    assert_eq!(envelope["meta"]["provider"], "mock");

    // RFC 0007 R2: stderr is completely silent in JSON mode — no progress,
    // and no spurious warnings from our own operational env vars either.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.trim(), "", "stderr must be silent in JSON mode");
}

#[test]
fn batch_size_splits_the_run_and_is_reported_in_meta() {
    let (fx, short) = seeded_repo();
    // Two batches (2 + 1): the script provides one response per request.
    let script = json!([
        { "critiques": [
            { "sha": short[2], "score": 9, "why_good": "solid",
              "tags": { "vague": false, "misleading": false, "no_why": false } },
            { "sha": short[1], "score": 4, "issue": "vague", "better": "be specific",
              "tags": { "vague": true, "misleading": false, "no_why": true } },
        ]},
        { "critiques": [
            { "sha": short[0], "score": 1, "issue": "wip", "better": "say what",
              "tags": { "vague": true, "misleading": false, "no_why": true } },
        ]},
    ]);
    let output = gitalyzer(&fx, &script)
        .args(["analyze", "--format", "json", "--batch-size", "2"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("clean JSON stdout");
    assert_eq!(envelope["meta"]["batches"], 2);
}

#[test]
fn output_flag_writes_the_report_to_a_file() {
    let (fx, short) = seeded_repo();
    let report_path = fx.path().join("report.json");

    // JSON + --output: file gets the document, stdout stays empty.
    let output = gitalyzer(&fx, &critiques(&short))
        .args(["analyze", "--format", "json", "--output"])
        .arg(&report_path)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "stdout must be empty when writing to a file"
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).expect("file"))
            .expect("valid JSON in file");
    assert_eq!(written["mode"], "analyze");

    // Human + --output: confirmation on stdout (RFC 0005 R9).
    gitalyzer(&fx, &critiques(&short))
        .args(["analyze", "--output"])
        .arg(fx.path().join("report.txt"))
        .assert()
        .success()
        .stdout(contains("Report written to"));
}

#[test]
fn incomplete_critiques_fail_with_an_actionable_error() {
    let (fx, short) = seeded_repo();
    // Only one commit critiqued; schema-valid, so the repair retry does not
    // trigger — the mismatch is caught at merge time. Two entries feed the
    // repairless second validation… none needed: one response suffices.
    let script = json!([{ "critiques": [
        { "sha": short[0], "score": 1, "issue": "x", "better": "y",
          "tags": { "vague": true, "misleading": false, "no_why": true } },
    ]}]);
    gitalyzer(&fx, &script)
        .arg("analyze")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("critiqued 1 of 3 commits"));
}
