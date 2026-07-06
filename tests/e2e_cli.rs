//! End-to-end CLI tests (RFC 0008 slice 1): help behavior, usage errors, and
//! configuration failures observed through the real binary.
//!
//! Every command runs with a scrubbed environment and a temp working
//! directory, so the developer's own `~/.config/gitalyzer` or `GITALYZER_*`
//! variables can never leak into assertions.

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

/// A `gitalyzer` command isolated from the host environment.
fn gitalyzer(home: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("gitalyzer").expect("binary builds");
    cmd.current_dir(home.path())
        .env_clear()
        .env("HOME", home.path());
    cmd
}

#[test]
fn help_lists_subcommands_and_examples() {
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("analyze"))
        .stdout(contains("write"))
        .stdout(contains("Examples:"));
}

#[test]
fn bare_invocation_prints_help_and_exits_zero() {
    // RFC 0001 R2: help to stdout, exit 0 — not a usage error.
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .assert()
        .success()
        .stdout(contains("Usage:"));
}

#[test]
fn version_flag_reports_the_crate_version() {
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn branch_without_url_is_a_usage_error() {
    // RFC 0001 R4: --branch requires --url → clap usage error, exit 2.
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .args(["analyze", "--branch", "develop"])
        .assert()
        .failure()
        .code(2)
        .stderr(contains("--url"));
}

#[test]
fn missing_explicit_config_file_fails_with_exit_one() {
    // RFC 0002 R4: an explicit --config file must exist.
    let home = TempDir::new().expect("tempdir");
    let absent = home.path().join("absent.yaml");
    gitalyzer(&home)
        .args(["analyze", "--config"])
        .arg(&absent)
        .assert()
        .failure()
        .code(1)
        .stderr(contains("failed to load configuration"));
}

#[test]
fn invalid_env_value_fails_with_exit_one() {
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .arg("analyze")
        .env("GITALYZER_ANALYZE__COUNT", "abc")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("invalid"));
}

#[test]
fn unknown_provider_fails_validation_with_exit_one() {
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .arg("analyze")
        .env("GITALYZER_PROVIDER", "mistral")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("mistral"));
}

#[test]
fn analyze_outside_a_repository_fails_actionably() {
    // RFC 0004 R8: the temp home is not a Git repository.
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .arg("analyze")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("not inside a Git repository"));
}

#[test]
fn write_reports_not_implemented_for_now() {
    // Slice honesty: `write` parses and resolves config, then says so.
    let home = TempDir::new().expect("tempdir");
    gitalyzer(&home)
        .arg("write")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("not implemented yet"));
}
