//! Layered configuration (RFC 0002).
//!
//! Precedence, low → high, merged key-by-key (RFC 0002 R3):
//!
//! 1. built-in defaults ([`Settings::default`])
//! 2. user config file (`$XDG_CONFIG_HOME/gitalyzer/config.yaml`, defaulting to
//!    `~/.config/gitalyzer/config.yaml`)
//! 3. project config file (`.gitalyzer.yaml` at the repository root)
//! 4. environment variables (`GITALYZER_*`, `__` as nesting separator)
//! 5. CLI flags ([`CliOverrides`])
//!
//! `--config <path>` replaces file discovery entirely (RFC 0002 R4); the
//! explicit file must exist. Standard provider credential variables
//! (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) act as API-key fallbacks
//! (RFC 0002 R6, RFC 0003 R6).

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

use config::{Config, Environment, File, FileFormat};
use serde::{Deserialize, Serialize};

/// Prefix of every Gitalyzer environment variable (RFC 0002 R5).
pub const ENV_PREFIX: &str = "GITALYZER";
/// Nesting separator inside environment variable names (RFC 0002 R5).
pub const ENV_SEPARATOR: &str = "__";
/// Project-level configuration file name, looked up at the repository root.
pub const PROJECT_CONFIG_FILE: &str = ".gitalyzer.yaml";

/// Standard (non-namespaced) credential variables honored as `api_key`
/// fallbacks per provider (RFC 0003 R6).
const API_KEY_FALLBACKS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
];

/// `GITALYZER_*` variables that are operational, not configuration — they
/// must never reach the env layer (they would read as unknown config keys
/// and warn, polluting even JSON-mode stderr, RFC 0007 R2).
/// `GITALYZER_ASSUME_TTY` is the internal end-to-end-testing escape hatch for
/// the write-mode TTY requirement (RFC 0006 R1).
const RESERVED_ENV_VARS: &[&str] = &[
    "GITALYZER_LOG",
    "GITALYZER_LOG_FORMAT",
    "GITALYZER_MOCK_SCRIPT",
    "GITALYZER_ASSUME_TTY",
];

/// Configuration keys understood by this version; anything else in the merged
/// configuration produces a warning, not an error (RFC 0002 R8).
const KNOWN_KEYS: &[&str] = &[
    "provider",
    "model",
    "request_timeout_secs",
    "analyze.count",
    "analyze.batch_size",
    "analyze.concurrency",
    "analyze.max_patch_bytes",
    "analyze.max_batch_bytes",
    "analyze.system_prompt",
    "analyze.thresholds.needs_work",
    "analyze.thresholds.well_written",
    "write.style",
    "write.system_prompt",
    "write.max_file_patch_bytes",
    "write.max_diff_bytes",
    "providers.anthropic.api_key",
    "providers.anthropic.base_url",
    "providers.openai.api_key",
    "providers.openai.base_url",
];

/// Errors produced while loading or validating configuration (RFC 0002 R8).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Reading, parsing, or deserializing any configuration source failed.
    #[error(transparent)]
    Load(#[from] config::ConfigError),
    /// The merged configuration is semantically invalid.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// Fully resolved settings; field defaults are the built-in defaults layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Active LLM provider id (RFC 0003 R1).
    pub provider: String,
    /// Model override; `None` resolves to the provider's default (RFC 0003 R7).
    pub model: Option<String>,
    /// HTTP request timeout for provider calls, seconds (RFC 0003 R9).
    pub request_timeout_secs: u64,
    /// Analysis-mode settings (RFC 0005).
    pub analyze: AnalyzeSettings,
    /// Write-mode settings (RFC 0006).
    pub write: WriteSettings,
    /// Per-provider connection settings (RFC 0003).
    pub providers: Providers,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "anthropic".into(),
            model: None,
            request_timeout_secs: 120,
            analyze: AnalyzeSettings::default(),
            write: WriteSettings::default(),
            providers: Providers::default(),
        }
    }
}

/// Analysis-mode settings (RFC 0005 R3–R5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyzeSettings {
    /// Commits analyzed by default (RFC 0001 R3).
    pub count: u32,
    /// Commits per LLM request; `0` = one request for everything (RFC 0005 R4).
    pub batch_size: u32,
    /// Batch requests in flight; `1` = sequential (RFC 0005 R4).
    pub concurrency: u32,
    /// Per-commit patch excerpt cap in bytes; `0` disables patch content
    /// entirely (RFC 0005 R3).
    pub max_patch_bytes: u64,
    /// Hard byte ceiling per LLM request; batches are packed to stay under
    /// it regardless of `batch_size`, so huge ranges cannot overflow a
    /// model's context window (RFC 0005 R4, amended).
    pub max_batch_bytes: u64,
    /// Replace the built-in critique system prompt (RFC 0005 R2, amended);
    /// `null` uses the built-in rubric. Structured output stays
    /// schema-enforced regardless.
    pub system_prompt: Option<String>,
    /// Report bucket thresholds (RFC 0005 R5).
    pub thresholds: Thresholds,
}

impl Default for AnalyzeSettings {
    fn default() -> Self {
        Self {
            count: 50,
            batch_size: 10,
            concurrency: 1,
            max_patch_bytes: 4096,
            max_batch_bytes: 262_144,
            system_prompt: None,
            thresholds: Thresholds::default(),
        }
    }
}

/// Score thresholds for report buckets (RFC 0005 R5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// Scores `<=` this land in the 💩 needs-work section.
    pub needs_work: u8,
    /// Scores `>=` this land in the 💎 well-written section.
    pub well_written: u8,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            needs_work: 5,
            well_written: 8,
        }
    }
}

/// Write-mode settings (RFC 0006 R3–R4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteSettings {
    /// Suggested-message style (RFC 0006 R4).
    pub style: Style,
    /// Replace the built-in suggestion system prompt (RFC 0006 R5, amended);
    /// `null` uses the built-in one. The style clause (R4) is still appended.
    pub system_prompt: Option<String>,
    /// Per-file staged patch cap in bytes (RFC 0006 R3).
    pub max_file_patch_bytes: u64,
    /// Total staged patch budget in bytes (RFC 0006 R3).
    pub max_diff_bytes: u64,
}

impl Default for WriteSettings {
    fn default() -> Self {
        Self {
            style: Style::Auto,
            system_prompt: None,
            max_file_patch_bytes: 8192,
            max_diff_bytes: 65536,
        }
    }
}

/// Commit message style for suggestions (RFC 0006 R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Infer the repository's dominant convention; fall back to Conventional
    /// Commits when history is absent or inconsistent.
    #[default]
    Auto,
    /// Always Conventional Commits (`type(scope): summary` + bullets).
    Conventional,
}

/// Connection settings for the known providers (RFC 0003 R1).
///
/// Unknown provider ids are a hard error (RFC 0002 R8), hence
/// `deny_unknown_fields` here while the rest of the tree merely warns.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Providers {
    /// Native Anthropic API (RFC 0003 R5).
    pub anthropic: ProviderSettings,
    /// `OpenAI` API or any OpenAI-compatible endpoint (RFC 0003 R5).
    pub openai: ProviderSettings,
}

/// Settings for a single provider (RFC 0003 R5–R6).
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderSettings {
    /// API key; prefer environment variables over config files (RFC 0002 R7).
    pub api_key: Option<String>,
    /// Endpoint override; enables OpenAI-compatible servers (RFC 0003 R5).
    pub base_url: Option<String>,
}

impl fmt::Debug for ProviderSettings {
    /// Manual `Debug` so API keys can never leak into logs (RFC 0007 R7).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderSettings")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// A configuration file to load, and whether its absence is an error.
#[derive(Debug, Clone)]
pub struct FileSource {
    /// Path of the YAML file.
    pub path: PathBuf,
    /// `true` for `--config` (RFC 0002 R4); discovered files are optional.
    pub required: bool,
}

/// The concrete sources a [`load`] call reads from.
///
/// Tests construct this directly (with an env snapshot injected) so precedence
/// can be exercised without touching process globals.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// Config files, lowest precedence first.
    pub files: Vec<FileSource>,
    /// Environment snapshot; `None` reads the real process environment.
    pub env: Option<HashMap<String, String>>,
}

impl Sources {
    /// Discover sources per RFC 0002 R2/R4: an explicit `--config` path
    /// replaces discovery and must exist; otherwise the optional user and
    /// project files are layered in that order.
    pub fn discover(explicit: Option<&Path>) -> Self {
        if let Some(path) = explicit {
            return Self {
                files: vec![FileSource {
                    path: path.to_owned(),
                    required: true,
                }],
                env: None,
            };
        }
        let mut files = Vec::new();
        if let Some(user) = user_config_path() {
            files.push(FileSource {
                path: user,
                required: false,
            });
        }
        if let Some(root) = env::current_dir().ok().and_then(|cwd| find_repo_root(&cwd)) {
            files.push(FileSource {
                path: root.join(PROJECT_CONFIG_FILE),
                required: false,
            });
        }
        Self { files, env: None }
    }

    /// Read a single variable from the snapshot or the real environment.
    fn env_get(&self, key: &str) -> Option<String> {
        match &self.env {
            Some(map) => map.get(key).cloned(),
            None => env::var(key).ok(),
        }
    }
}

/// `$XDG_CONFIG_HOME/gitalyzer/config.yaml`, defaulting to
/// `~/.config/gitalyzer/config.yaml`, uniformly on all platforms (RFC 0002 R2).
fn user_config_path() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")))?;
    Some(base.join("gitalyzer").join("config.yaml"))
}

/// Home directory via `HOME` (Unix) or `USERPROFILE` (Windows).
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Nearest ancestor of `start` that contains a `.git` entry (dir or file).
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Load and merge all layers below the CLI (RFC 0002 R3), warn about unknown
/// keys (R8), and apply standard credential fallbacks (R6).
///
/// CLI overrides are applied afterwards via [`Settings::apply`]; call
/// [`Settings::validate`] once everything is merged.
pub fn load(sources: &Sources) -> Result<Settings, ConfigError> {
    let mut builder = Config::builder().add_source(Config::try_from(&Settings::default())?);
    for file in &sources.files {
        builder = builder.add_source(
            File::from(file.path.clone())
                .format(FileFormat::Yaml)
                .required(file.required),
        );
    }
    // The env layer always runs off an explicit snapshot (injected by tests
    // or collected here) so reserved operational variables can be stripped.
    let mut snapshot: HashMap<String, String> = match &sources.env {
        Some(injected) => injected.clone(),
        None => env::vars().collect(),
    };
    snapshot.retain(|key, _| !RESERVED_ENV_VARS.contains(&key.as_str()));

    // `prefix_separator` must be pinned: with a nesting separator configured,
    // config-rs would otherwise expect `GITALYZER__…` instead of the RFC 0002
    // convention `GITALYZER_SECTION__KEY`. Empty variables count as unset —
    // CI systems export blank strings for optional parameters.
    let env_source = Environment::with_prefix(ENV_PREFIX)
        .prefix_separator("_")
        .separator(ENV_SEPARATOR)
        .try_parsing(true)
        .ignore_empty(true)
        .source(Some(snapshot));
    let merged = builder.add_source(env_source).build()?;

    warn_unknown_keys(&merged);

    let mut settings: Settings = merged.try_deserialize()?;
    apply_api_key_fallbacks(&mut settings, sources);
    Ok(settings)
}

/// Fill missing `api_key`s from the standard provider variables (RFC 0003 R6).
fn apply_api_key_fallbacks(settings: &mut Settings, sources: &Sources) {
    for (id, var) in API_KEY_FALLBACKS {
        let slot = match *id {
            "anthropic" => &mut settings.providers.anthropic.api_key,
            "openai" => &mut settings.providers.openai.api_key,
            _ => unreachable!("fallback table only lists known providers"),
        };
        if slot.is_none() {
            *slot = sources.env_get(var);
        }
    }
}

/// Emit a warning for every merged leaf key this version does not understand
/// (RFC 0002 R8). Never fails: on any hiccup it simply stays silent.
fn warn_unknown_keys(merged: &Config) {
    let Ok(value) = merged.clone().try_deserialize::<serde_json::Value>() else {
        return;
    };
    let mut path = String::new();
    walk_leaves(&value, &mut path, &mut |leaf| {
        if !KNOWN_KEYS.contains(&leaf) {
            tracing::warn!("unknown configuration key `{leaf}` (ignored)");
        }
    });
}

/// Depth-first traversal invoking `on_leaf` with dotted paths of leaf values.
fn walk_leaves(value: &serde_json::Value, path: &mut String, on_leaf: &mut impl FnMut(&str)) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let previous = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                walk_leaves(child, path, on_leaf);
                path.truncate(previous);
            }
        }
        _ => on_leaf(path),
    }
}

/// CLI-sourced overrides — the highest precedence layer (RFC 0002 R3).
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    /// `--provider` (RFC 0001 R7).
    pub provider: Option<String>,
    /// `--model` (RFC 0001 R7).
    pub model: Option<String>,
    /// `analyze -n/--count` (RFC 0001 R4).
    pub count: Option<u32>,
    /// `analyze --batch-size` (RFC 0001 R4).
    pub batch_size: Option<u32>,
}

impl Settings {
    /// Overlay CLI flags onto the merged settings (RFC 0002 R3, step 5).
    pub fn apply(&mut self, overrides: &CliOverrides) {
        if let Some(provider) = &overrides.provider {
            self.provider.clone_from(provider);
        }
        if let Some(model) = &overrides.model {
            self.model = Some(model.clone());
        }
        if let Some(count) = overrides.count {
            self.analyze.count = count;
        }
        if let Some(batch_size) = overrides.batch_size {
            self.analyze.batch_size = batch_size;
        }
    }

    /// Semantic validation of the fully merged settings (RFC 0002 R8,
    /// RFC 0005 R5). Messages are actionable and name the offending key.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // `mock` is the internal deterministic test provider (RFC 0007 R11);
        // deliberately absent from the user-facing error message.
        if !matches!(self.provider.as_str(), "anthropic" | "openai" | "mock") {
            return Err(ConfigError::Invalid(format!(
                "provider: unknown id `{}` (expected `anthropic` or `openai`)",
                self.provider
            )));
        }
        if self.analyze.count == 0 {
            return Err(ConfigError::Invalid(
                "analyze.count must be at least 1".into(),
            ));
        }
        if self.analyze.concurrency == 0 {
            return Err(ConfigError::Invalid(
                "analyze.concurrency must be at least 1 (1 = sequential)".into(),
            ));
        }
        if self.request_timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "request_timeout_secs must be at least 1".into(),
            ));
        }
        let thresholds = &self.analyze.thresholds;
        for (name, value) in [
            ("needs_work", thresholds.needs_work),
            ("well_written", thresholds.well_written),
        ] {
            if !(1..=10).contains(&value) {
                return Err(ConfigError::Invalid(format!(
                    "analyze.thresholds.{name} must be within 1..=10 (got {value})"
                )));
            }
        }
        if thresholds.needs_work >= thresholds.well_written {
            return Err(ConfigError::Invalid(format!(
                "analyze.thresholds: needs_work ({}) must be lower than well_written ({})",
                thresholds.needs_work, thresholds.well_written
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_settings_debug_redacts_api_key() {
        let settings = ProviderSettings {
            api_key: Some("sk-super-secret".into()),
            base_url: Some("https://example.test".into()),
        };
        let rendered = format!("{settings:?}");
        assert!(
            !rendered.contains("sk-super-secret"),
            "key leaked: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn discover_with_explicit_path_replaces_discovery() {
        let sources = Sources::discover(Some(Path::new("/tmp/custom.yaml")));
        assert_eq!(sources.files.len(), 1);
        assert!(sources.files[0].required);
        assert_eq!(sources.files[0].path, Path::new("/tmp/custom.yaml"));
    }
}
