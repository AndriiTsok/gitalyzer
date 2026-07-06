//! Integration tests for the RFC 0002 precedence chain, exercised through the
//! library API with injected environment snapshots (no process globals).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gitalyzer::config::{self, CliOverrides, FileSource, Settings, Sources, Style};

/// Write a YAML file into `dir` and return its path.
fn write_yaml(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("fixture yaml should be writable");
    path
}

/// Build an injected env snapshot from key/value pairs.
fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// Sources with the given optional files (low → high) and env snapshot.
fn sources(files: &[&Path], pairs: &[(&str, &str)]) -> Sources {
    Sources {
        files: files
            .iter()
            .map(|path| FileSource {
                path: (*path).to_owned(),
                required: false,
            })
            .collect(),
        env: Some(env_map(pairs)),
    }
}

#[test]
fn built_in_defaults_apply_when_nothing_is_provided() {
    let loaded = config::load(&sources(&[], &[])).expect("defaults should load");
    assert_eq!(loaded, Settings::default());
    assert_eq!(loaded.provider, "anthropic");
    assert_eq!(loaded.analyze.count, 50);
    assert_eq!(loaded.analyze.batch_size, 10);
    assert_eq!(loaded.analyze.concurrency, 1);
    assert_eq!(loaded.analyze.thresholds.needs_work, 5);
    assert_eq!(loaded.analyze.thresholds.well_written, 8);
    assert_eq!(loaded.write.style, Style::Auto);
    assert_eq!(loaded.request_timeout_secs, 120);
}

#[test]
fn project_file_overrides_user_file_key_by_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let user = write_yaml(
        dir.path(),
        "user.yaml",
        "provider: openai\nanalyze:\n  count: 10\n",
    );
    let project = write_yaml(
        dir.path(),
        "project.yaml",
        "analyze:\n  count: 20\n  batch_size: 3\n",
    );

    let loaded = config::load(&sources(&[&user, &project], &[])).expect("load");

    // Deep merge: the project file wins only on the keys it sets.
    assert_eq!(loaded.provider, "openai", "user-level value must survive");
    assert_eq!(loaded.analyze.count, 20, "project overrides user");
    assert_eq!(loaded.analyze.batch_size, 3);
    assert_eq!(
        loaded.analyze.concurrency, 1,
        "untouched keys keep defaults"
    );
}

#[test]
fn env_overrides_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = write_yaml(dir.path(), "cfg.yaml", "analyze:\n  count: 20\n");

    let loaded =
        config::load(&sources(&[&file], &[("GITALYZER_ANALYZE__COUNT", "99")])).expect("load");

    assert_eq!(loaded.analyze.count, 99);
}

#[test]
fn cli_overrides_env() {
    let mut loaded =
        config::load(&sources(&[], &[("GITALYZER_ANALYZE__COUNT", "99")])).expect("load");
    loaded.apply(&CliOverrides {
        count: Some(7),
        ..CliOverrides::default()
    });
    assert_eq!(loaded.analyze.count, 7);
}

#[test]
fn nested_env_names_map_through_double_underscores() {
    let loaded = config::load(&sources(
        &[],
        &[
            ("GITALYZER_PROVIDER", "openai"),
            ("GITALYZER_ANALYZE__MAX_PATCH_BYTES", "0"),
            ("GITALYZER_WRITE__STYLE", "conventional"),
            (
                "GITALYZER_PROVIDERS__OPENAI__BASE_URL",
                "http://localhost:11434/v1",
            ),
        ],
    ))
    .expect("load");

    assert_eq!(loaded.provider, "openai");
    assert_eq!(loaded.analyze.max_patch_bytes, 0);
    assert_eq!(loaded.write.style, Style::Conventional);
    assert_eq!(
        loaded.providers.openai.base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
}

#[test]
fn standard_provider_variable_is_api_key_fallback() {
    let loaded =
        config::load(&sources(&[], &[("ANTHROPIC_API_KEY", "sk-standard")])).expect("load");
    assert_eq!(
        loaded.providers.anthropic.api_key.as_deref(),
        Some("sk-standard")
    );
}

#[test]
fn namespaced_api_key_beats_standard_variable() {
    let loaded = config::load(&sources(
        &[],
        &[
            ("ANTHROPIC_API_KEY", "sk-standard"),
            ("GITALYZER_PROVIDERS__ANTHROPIC__API_KEY", "sk-namespaced"),
        ],
    ))
    .expect("load");
    assert_eq!(
        loaded.providers.anthropic.api_key.as_deref(),
        Some("sk-namespaced")
    );
}

#[test]
fn unknown_provider_id_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = write_yaml(
        dir.path(),
        "cfg.yaml",
        "providers:\n  custom:\n    api_key: x\n",
    );

    let error = config::load(&sources(&[&file], &[])).expect_err("unknown provider must fail");
    assert!(
        error.to_string().contains("custom"),
        "actionable message, got: {error}"
    );
}

#[test]
fn invalid_value_type_is_an_error() {
    let error = config::load(&sources(&[], &[("GITALYZER_ANALYZE__COUNT", "abc")]))
        .expect_err("non-numeric count must fail");
    assert!(error.to_string().contains("invalid"), "got: {error}");
}

#[test]
fn missing_explicit_config_file_is_an_error() {
    let missing = Sources {
        files: vec![FileSource {
            path: PathBuf::from("/definitely/absent.yaml"),
            required: true,
        }],
        env: Some(env_map(&[])),
    };
    config::load(&missing).expect_err("explicit --config file must exist (RFC 0002 R4)");
}

#[test]
fn threshold_ordering_is_validated() {
    let mut settings = config::load(&sources(
        &[],
        &[
            ("GITALYZER_ANALYZE__THRESHOLDS__NEEDS_WORK", "9"),
            ("GITALYZER_ANALYZE__THRESHOLDS__WELL_WRITTEN", "3"),
        ],
    ))
    .expect("load succeeds; validation is separate");
    let error = settings
        .validate()
        .expect_err("inverted thresholds must fail");
    assert!(error.to_string().contains("needs_work"), "got: {error}");

    settings.analyze.thresholds.needs_work = 5;
    settings.analyze.thresholds.well_written = 8;
    settings.validate().expect("defaults are valid");
}

#[test]
fn unknown_provider_id_in_cli_override_fails_validation() {
    let mut settings = config::load(&sources(&[], &[])).expect("load");
    settings.apply(&CliOverrides {
        provider: Some("mistral".into()),
        ..CliOverrides::default()
    });
    let error = settings
        .validate()
        .expect_err("unknown provider id must fail");
    assert!(error.to_string().contains("mistral"), "got: {error}");
}
