//! Write mode (RFC 0006): turn the staged changes into a well-formed commit
//! message suggestion, under context budgeting that never fails on size.
//!
//! The interactive accept/type/regenerate loop lives in the binary; this
//! module owns everything testable: staged-context assembly (R2–R3), style
//! resolution (R4), the suggestion task (R5), and the JSON envelope (R10).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{Settings, Style};
use crate::git::{GitError, Repo, StagedChanges, staged_changes};
use crate::provider::{AnyProvider, LlmProvider as _, ProviderError, run_task};

/// Tool/schema name of the suggestion task.
const TASK_NAME: &str = "suggest_commit_message";
/// Tool description shown to the model.
const TASK_DESCRIPTION: &str =
    "Record a commit message suggestion for the staged changes, with detected change themes";
/// Recent subjects offered as style context in `auto` mode (RFC 0006 R4).
const STYLE_CONTEXT_SUBJECTS: usize = 15;

/// File names and path fragments whose patch content is never sent — always
/// listed, content omitted (RFC 0006 R3).
const GENERATED_FILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "composer.lock",
    "Gemfile.lock",
    "poetry.lock",
    "uv.lock",
    "go.sum",
];
/// Directory prefixes treated as generated/vendored (RFC 0006 R3).
const GENERATED_DIRS: &[&str] = &["node_modules/", "vendor/", "dist/", "build/", "target/"];
/// Suffixes treated as generated/minified (RFC 0006 R3).
const GENERATED_SUFFIXES: &[&str] = &[".min.js", ".min.css", ".map", ".lock"];

/// The base system prompt (RFC 0006 R5); a style clause is appended (R4).
const SYSTEM_PROMPT: &str = "\
You are an expert developer writing a Git commit message for the staged changes
provided by the user.

Return `changes_detected` — up to 5 short bullets naming the change themes —
plus the message itself as `subject` and optional `body`.

Rules: the subject is imperative and at most 72 characters; the body is a
short bullet list explaining what changed and why, omitted (null) when the
change is trivial. Some patch content may be truncated or omitted (markers are
shown); describe only what you can actually see.";

/// The model's suggestion (RFC 0006 R5).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Suggestion {
    /// Up to 5 short bullets naming the change themes.
    pub changes_detected: Vec<String>,
    /// Imperative subject line, at most 72 characters.
    pub subject: String,
    /// Explanatory bullet list; null for trivial changes.
    #[serde(default)]
    pub body: Option<String>,
}

impl Suggestion {
    /// The full commit message: subject plus blank-line-separated body.
    pub fn message(&self) -> String {
        match self.body.as_deref().map(str::trim) {
            Some(body) if !body.is_empty() => format!("{}\n\n{body}", self.subject),
            _ => self.subject.clone(),
        }
    }
}

/// Staged summary in the JSON envelope (RFC 0006 R10).
#[derive(Debug, Clone, Serialize)]
pub struct StagedSummary {
    /// Files staged.
    pub files_changed: usize,
    /// Lines added.
    pub insertions: usize,
    /// Lines removed.
    pub deletions: usize,
    /// Paths, sorted.
    pub files: Vec<String>,
}

/// Suggestion as reported in the envelope (RFC 0006 R10).
#[derive(Debug, Clone, Serialize)]
pub struct SuggestionReport {
    /// Subject line.
    pub subject: String,
    /// Optional body.
    pub body: Option<String>,
    /// The configured style (`auto` or `conventional`).
    pub style: &'static str,
}

/// The write-mode JSON envelope (RFC 0006 R10).
#[derive(Debug, Clone, Serialize)]
pub struct WriteReport {
    /// Envelope version.
    pub schema_version: u32,
    /// Always `write`.
    pub mode: &'static str,
    /// Staged summary.
    pub staged: StagedSummary,
    /// Detected change themes.
    pub changes_detected: Vec<String>,
    /// The suggestion.
    pub suggestion: SuggestionReport,
    /// Provider/model facts.
    pub meta: Meta,
}

/// Provider/model facts (RFC 0006 R10).
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    /// Provider id used.
    pub provider: String,
    /// Model used.
    pub model: String,
}

/// A prepared write session: staged context assembled once, reused across
/// regenerations (RFC 0006 R6).
#[derive(Debug)]
pub struct WriteSession {
    /// Extracted staged changes.
    pub staged: StagedChanges,
    system: String,
    user: String,
    style: Style,
}

impl WriteSession {
    /// Extract staged changes and assemble the budgeted context (R1–R4).
    pub fn prepare(repo: &Repo, settings: &Settings) -> Result<Self, GitError> {
        let staged = staged_changes(repo, settings.write.max_file_patch_bytes)?;
        let subjects = match settings.write.style {
            Style::Auto => repo.recent_subjects(STYLE_CONTEXT_SUBJECTS)?,
            Style::Conventional => Vec::new(),
        };
        let system = format!(
            "{SYSTEM_PROMPT}\n\n{}",
            style_clause(settings.write.style, &subjects)
        );
        let user = build_context(&staged, settings.write.max_diff_bytes);
        Ok(Self {
            staged,
            system,
            user,
            style: settings.write.style,
        })
    }

    /// Ask the provider for a suggestion; `previous` requests a distinct
    /// alternative (the regenerate path, R6).
    pub async fn suggest(
        &self,
        provider: &AnyProvider,
        previous: Option<&Suggestion>,
    ) -> Result<Suggestion, ProviderError> {
        let user = match previous {
            None => self.user.clone(),
            Some(prior) => format!(
                "{}\n\nYou previously suggested:\n{}\n\nProduce a distinctly different, \
                 better alternative.",
                self.user,
                prior.message()
            ),
        };
        run_task::<Suggestion>(provider, TASK_NAME, TASK_DESCRIPTION, &self.system, &user).await
    }

    /// Build the JSON envelope for a suggestion (R10).
    pub fn report(&self, suggestion: &Suggestion, provider: &AnyProvider) -> WriteReport {
        WriteReport {
            schema_version: 1,
            mode: "write",
            staged: StagedSummary {
                files_changed: self.staged.stats.files_changed,
                insertions: self.staged.stats.insertions,
                deletions: self.staged.stats.deletions,
                files: self.staged.stats.files.clone(),
            },
            changes_detected: suggestion.changes_detected.clone(),
            suggestion: SuggestionReport {
                subject: suggestion.subject.clone(),
                body: suggestion.body.clone(),
                style: style_name(self.style),
            },
            meta: Meta {
                provider: provider.id().to_owned(),
                model: provider.model().to_owned(),
            },
        }
    }
}

/// The configured style as its config-file spelling.
pub fn style_name(style: Style) -> &'static str {
    match style {
        Style::Auto => "auto",
        Style::Conventional => "conventional",
    }
}

/// The style clause appended to the system prompt (RFC 0006 R4).
fn style_clause(style: Style, subjects: &[String]) -> String {
    match style {
        Style::Conventional => {
            "Style: always use Conventional Commits — `type(scope): summary`.".to_owned()
        }
        Style::Auto if subjects.is_empty() => {
            "Style: the repository has no usable history; use Conventional Commits — \
             `type(scope): summary`."
                .to_owned()
        }
        Style::Auto => format!(
            "Style: match the repository's dominant message convention if one is discernible \
             from these recent subjects; otherwise use Conventional Commits \
             (`type(scope): summary`).\nRecent subjects:\n{}",
            subjects
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

/// Whether a path's content is generated/vendored/lock material whose patch
/// content is never sent (RFC 0006 R3).
fn is_generated(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    GENERATED_FILES.contains(&name)
        || GENERATED_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || GENERATED_DIRS
            .iter()
            .any(|dir| path.starts_with(dir) || path.contains(&format!("/{dir}")))
}

/// Assemble the user prompt under the total budget (RFC 0006 R2–R3):
/// stats and the full file list always; patch content per file only while it
/// fits, dropping the largest files' content first; explicit markers for
/// everything omitted. By construction this cannot fail on size.
fn build_context(staged: &StagedChanges, max_diff_bytes: u64) -> String {
    use std::fmt::Write as _;

    let budget = usize::try_from(max_diff_bytes).unwrap_or(usize::MAX);

    // Which files get patch content: non-generated ones, smallest first,
    // while the running total fits the budget (largest are dropped first).
    let mut candidates: Vec<(usize, usize)> = staged
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| !is_generated(&file.path))
        .filter_map(|(index, file)| file.patch.as_ref().map(|patch| (index, patch.len())))
        .collect();
    candidates.sort_by_key(|(_, len)| *len);
    let mut included = vec![false; staged.files.len()];
    let mut used = 0usize;
    for (index, len) in candidates {
        if used + len <= budget {
            included[index] = true;
            used += len;
        }
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "Staged changes: {} file(s), +{} -{}\n\nFiles:",
        staged.stats.files_changed, staged.stats.insertions, staged.stats.deletions,
    );
    for file in &staged.files {
        let _ = write!(
            out,
            "- {} ({:?}, +{} -{})",
            file.path, file.status, file.insertions, file.deletions
        );
        if file.binary {
            out.push_str(" [binary]");
        }
        if is_generated(&file.path) {
            out.push_str(" [content omitted: generated/lock file]");
        }
        out.push('\n');
    }

    out.push_str("\nPatches:\n");
    for (index, file) in staged.files.iter().enumerate() {
        if is_generated(&file.path) {
            continue;
        }
        match (&file.patch, included[index]) {
            (Some(patch), true) => {
                let _ = writeln!(out, "--- {} ---", file.path);
                out.push_str(patch);
                if file.patch_truncated {
                    out.push_str("\n[... file patch truncated at the per-file cap]\n");
                } else {
                    out.push('\n');
                }
            }
            (Some(_), false) => {
                let _ = writeln!(
                    out,
                    "--- {} --- [patch omitted: total budget reached]",
                    file.path
                );
            }
            (None, _) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffStat, FileStatus, StagedFile};

    fn file(path: &str, patch_text: Option<&str>) -> StagedFile {
        StagedFile {
            path: path.to_owned(),
            status: FileStatus::Modified,
            insertions: 1,
            deletions: 0,
            binary: false,
            patch: patch_text.map(str::to_owned),
            patch_truncated: false,
        }
    }

    fn staged(files: Vec<StagedFile>) -> StagedChanges {
        let stats = DiffStat {
            files_changed: files.len(),
            insertions: files.iter().map(|f| f.insertions).sum(),
            deletions: files.iter().map(|f| f.deletions).sum(),
            files: files.iter().map(|f| f.path.clone()).collect(),
        };
        StagedChanges { stats, files }
    }

    #[test]
    fn generated_paths_are_recognized() {
        for path in [
            "Cargo.lock",
            "web/package-lock.json",
            "assets/app.min.js",
            "node_modules/x/index.js",
            "crates/x/target/out.rs",
            "flake.lock",
        ] {
            assert!(is_generated(path), "{path} should be generated");
        }
        for path in ["src/main.rs", "docs/lockfile-notes.md", "locker.rs"] {
            assert!(!is_generated(path), "{path} should not be generated");
        }
    }

    #[test]
    fn generated_content_is_listed_but_never_sent() {
        let context = build_context(
            &staged(vec![
                file("src/lib.rs", Some("+real change\n")),
                file("Cargo.lock", Some("+dependency churn\n")),
            ]),
            1_000_000,
        );
        assert!(context.contains("- Cargo.lock"), "listed: {context}");
        assert!(context.contains("[content omitted: generated/lock file]"));
        assert!(
            !context.contains("dependency churn"),
            "content must be omitted"
        );
        assert!(context.contains("+real change"));
    }

    #[test]
    fn total_budget_drops_the_largest_patches_first() {
        let small = "s".repeat(10);
        let large = "L".repeat(100);
        let context = build_context(
            &staged(vec![
                file("large.rs", Some(&large)),
                file("small.rs", Some(&small)),
            ]),
            50,
        );
        assert!(context.contains(&small), "small patch fits");
        assert!(!context.contains(&large), "large patch dropped");
        assert!(context.contains("large.rs --- [patch omitted: total budget reached]"));
        // The file list always names everything (never-fail guarantee, R3).
        assert!(context.contains("- large.rs"));
    }

    #[test]
    fn zero_budget_still_produces_a_useful_context() {
        let context = build_context(&staged(vec![file("a.rs", Some("+x\n"))]), 0);
        assert!(context.contains("Staged changes: 1 file(s)"));
        assert!(context.contains("- a.rs"));
        assert!(
            !context.contains("+x"),
            "no patch content under a zero budget"
        );
    }

    #[test]
    fn style_clause_matches_configuration() {
        let conventional = style_clause(Style::Conventional, &[]);
        assert!(conventional.contains("always use Conventional Commits"));

        let auto_empty = style_clause(Style::Auto, &[]);
        assert!(auto_empty.contains("no usable history"));

        let subjects = vec!["feat(api): add cache".to_owned(), "fix: typo".to_owned()];
        let auto = style_clause(Style::Auto, &subjects);
        assert!(auto.contains("- feat(api): add cache"));
        assert!(auto.contains("dominant message convention"));
    }

    #[test]
    fn suggestion_message_joins_subject_and_body() {
        let with_body = Suggestion {
            changes_detected: vec![],
            subject: "feat: x".into(),
            body: Some("- detail".into()),
        };
        assert_eq!(with_body.message(), "feat: x\n\n- detail");

        let without = Suggestion {
            changes_detected: vec![],
            subject: "feat: x".into(),
            body: Some("   ".into()),
        };
        assert_eq!(without.message(), "feat: x");
    }
}
