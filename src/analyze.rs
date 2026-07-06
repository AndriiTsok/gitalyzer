//! Analysis mode orchestration (RFC 0005): batch commits per configuration,
//! run the rubric-driven critique task per batch (concurrently when asked),
//! merge critiques back onto commits, and compute deterministic local stats.

use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::git::{CommitInfo, GitError, HistoryOptions, Repo};
use crate::provider::{AnyProvider, LlmProvider as _, ProviderError, run_task};

/// Tool/schema name of the critique task.
const TASK_NAME: &str = "critique_commits";
/// Tool description shown to the model.
const TASK_DESCRIPTION: &str =
    "Record a quality critique for every Git commit message in the batch";
/// At most this many file paths are listed per commit in the prompt
/// (RFC 0005 R3); counts always reflect the full change.
const PROMPT_FILE_CAP: usize = 20;

/// The scoring rubric (RFC 0005 R2). Semantics are fixed by the RFC; the
/// wording lives here as the system prompt.
const SYSTEM_PROMPT: &str = "\
You are an expert code-review lead assessing Git commit message quality.

Score every commit from 1 to 10 against this rubric:
- 1-3: contentless (e.g. \"wip\", \"fixed bug\", \"update\")
- 4-5: vague or unscoped — real information is missing or imprecise
- 6-7: adequate — understandable, but could be sharper
- 8-10: specific, scoped, explains why, and matches the actual change

Judge these dimensions: specificity (what changed), rationale (why it changed),
conventional format (type(scope): summary), subject quality (imperative mood,
concise), and message-vs-diff fidelity using the provided diffstat and patch
excerpt.

For every commit return its sha and score, plus tags. For weak messages
(score 5 or lower) also return `issue` (what is wrong, concretely) and
`better` (a rewritten message that would earn a high score, grounded in the
actual change). For strong messages (score 8 or higher) return `why_good`.

Tags: `vague` (message lacks specifics), `misleading` (message does not match
the diff), `no_why` (no rationale is given or implied).";

/// One batch worth of critiques, as returned by the model (RFC 0005 R1).
#[derive(Debug, Deserialize, JsonSchema)]
struct BatchCritique {
    /// Exactly one critique per commit in the batch, in any order.
    critiques: Vec<CommitCritique>,
}

/// LLM judgment for a single commit (RFC 0005 R1).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CommitCritique {
    /// The sha of the commit being critiqued, echoed from the input.
    pub sha: String,
    /// Quality score, 1 (worst) to 10 (best).
    #[schemars(range(min = 1, max = 10))]
    pub score: u8,
    /// What is wrong with the message; required for scores of 5 or lower.
    #[serde(default)]
    pub issue: Option<String>,
    /// A rewritten message that would score highly; required for scores of 5
    /// or lower.
    #[serde(default)]
    pub better: Option<String>,
    /// What makes the message good; required for scores of 8 or higher.
    #[serde(default)]
    pub why_good: Option<String>,
    /// Boolean quality tags.
    #[serde(default)]
    pub tags: Tags,
}

/// LLM-judged boolean tags (RFC 0005 R1); `one_word` is deliberately computed
/// locally, never asked of the model.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
pub struct Tags {
    /// The message lacks specifics.
    #[serde(default)]
    pub vague: bool,
    /// The message does not match the actual diff.
    #[serde(default)]
    pub misleading: bool,
    /// No rationale is given or implied.
    #[serde(default)]
    pub no_why: bool,
}

/// Analysis-mode failures.
#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    /// Reading the repository failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// A provider call failed (after retries/repair per RFC 0003).
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The provider answered, but critiques were missing for some commits.
    #[error(
        "the provider critiqued {matched} of {expected} commits (missing: {missing}); \
         re-run, or lower analyze.batch_size"
    )]
    IncompleteCritique {
        /// Commits sent.
        expected: usize,
        /// Critiques matched back.
        matched: usize,
        /// Short shas without critiques.
        missing: String,
    },
}

/// Where the analyzed repository came from (RFC 0005 R8).
#[derive(Debug, Clone, Serialize)]
pub struct Repository {
    /// `local` or `remote`.
    pub source: &'static str,
    /// The Git URL for remote analysis.
    pub url: Option<String>,
}

impl Repository {
    /// The current working directory's repository.
    pub fn local() -> Self {
        Self {
            source: "local",
            url: None,
        }
    }

    /// A remote repository analyzed via `--url`.
    pub fn remote(url: String) -> Self {
        Self {
            source: "remote",
            url: Some(url),
        }
    }
}

/// History selection actually used (RFC 0005 R8).
#[derive(Debug, Clone, Serialize)]
pub struct Range {
    /// Start revision (`HEAD` when `--from` was not given).
    pub from: String,
    /// Commits requested.
    pub requested: u32,
    /// Commits actually analyzed (history may be shorter).
    pub analyzed: usize,
}

/// One fully analyzed commit: extraction + critique + local judgments.
#[derive(Debug, Clone, Serialize)]
pub struct ReportCommit {
    /// Full hex sha.
    pub sha: String,
    /// Short sha.
    pub short_sha: String,
    /// Author identity, `Name <email>`.
    pub author: String,
    /// Authored date, ISO 8601.
    pub date: String,
    /// Full commit message.
    pub message: String,
    /// Files touched.
    pub files_changed: usize,
    /// Lines added.
    pub insertions: usize,
    /// Lines removed.
    pub deletions: usize,
    /// LLM score, 1–10.
    pub score: u8,
    /// What is wrong (weak messages).
    pub issue: Option<String>,
    /// Suggested rewrite (weak messages).
    pub better: Option<String>,
    /// What makes it good (strong messages).
    pub why_good: Option<String>,
    /// LLM-judged tags.
    pub tags: Tags,
    /// Locally computed: the subject is a single word (RFC 0005 R1/R6).
    pub one_word: bool,
}

/// A tag count with its integer percentage (RFC 0005 R6).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Counted {
    /// Commits carrying the tag.
    pub count: usize,
    /// Rounded percentage over all analyzed commits.
    pub percent: usize,
}

/// Deterministic, locally computed statistics (RFC 0005 R6).
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    /// Mean score, rounded to one decimal.
    pub average_score: f64,
    /// LLM-tagged vague commits.
    pub vague: Counted,
    /// Locally detected one-word subjects.
    pub one_word: Counted,
}

/// Provider/model/batching facts about the run (RFC 0005 R8).
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    /// Provider id used.
    pub provider: String,
    /// Model used.
    pub model: String,
    /// Number of LLM requests made.
    pub batches: usize,
}

/// The complete analysis result — the single source both renderers consume
/// (RFC 0005 R7–R8).
#[derive(Debug, Clone, Serialize)]
pub struct AnalysisReport {
    /// Envelope version (RFC 0005 R8).
    pub schema_version: u32,
    /// Always `analyze`.
    pub mode: &'static str,
    /// Repository provenance.
    pub repository: Repository,
    /// History selection.
    pub range: Range,
    /// Every analyzed commit, newest first, regardless of report bucket.
    pub commits: Vec<ReportCommit>,
    /// Locally computed stats.
    pub stats: Stats,
    /// Run facts.
    pub meta: Meta,
}

/// Run analysis end-to-end over an already-discovered repository
/// (RFC 0005 pipeline). `repository` describes provenance for the report;
/// `on_batch_done` receives `(completed, total)` for progress indication
/// (RFC 0007 R1).
pub async fn run(
    repo: &Repo,
    provider: &AnyProvider,
    settings: &Settings,
    from: Option<String>,
    repository: Repository,
    on_batch_done: impl Fn(usize, usize) + Sync,
) -> Result<AnalysisReport, AnalyzeError> {
    let options = HistoryOptions {
        from: from.clone(),
        count: usize::try_from(settings.analyze.count).expect("u32 fits usize"),
        max_patch_bytes: settings.analyze.max_patch_bytes,
    };
    let commits = repo.history(&options)?;

    let batches = chunk(&commits, settings.analyze.batch_size);
    let batch_count = batches.len();
    tracing::debug!(
        commits = commits.len(),
        batches = batch_count,
        concurrency = settings.analyze.concurrency,
        "starting analysis"
    );

    let concurrency = usize::try_from(settings.analyze.concurrency.max(1)).expect("u32 fits");
    let completed = std::sync::atomic::AtomicUsize::new(0);
    let completed = &completed;
    let on_batch_done = &on_batch_done;
    let critiques: Vec<Vec<CommitCritique>> = stream::iter(batches.iter().enumerate())
        .map(|(index, batch)| async move {
            let user = batch_prompt(batch);
            tracing::debug!(
                batch = index + 1,
                of = batch_count,
                size = batch.len(),
                "critiquing batch"
            );
            let result = run_task::<BatchCritique>(
                provider,
                TASK_NAME,
                TASK_DESCRIPTION,
                SYSTEM_PROMPT,
                &user,
            )
            .await
            .map(|result| result.critiques);
            let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            on_batch_done(done, batch_count);
            result
        })
        .buffer_unordered(concurrency)
        // RFC 0005 R10: fail fast — the first batch error aborts the run.
        .try_collect()
        .await?;

    let critiques: Vec<CommitCritique> = critiques.into_iter().flatten().collect();
    let matched = match_critiques(&commits, &critiques)?;

    let report_commits: Vec<ReportCommit> = commits
        .iter()
        .zip(matched)
        .map(|(info, critique)| to_report_commit(info, critique))
        .collect();
    let stats = compute_stats(&report_commits);

    Ok(AnalysisReport {
        schema_version: 1,
        mode: "analyze",
        repository,
        range: Range {
            from: from.unwrap_or_else(|| "HEAD".to_owned()),
            requested: settings.analyze.count,
            analyzed: report_commits.len(),
        },
        commits: report_commits,
        stats,
        meta: Meta {
            provider: provider.id().to_owned(),
            model: provider.model().to_owned(),
            batches: batch_count,
        },
    })
}

/// Split commits into batches; `batch_size == 0` sends everything in one
/// request (RFC 0005 R4).
fn chunk(commits: &[CommitInfo], batch_size: u32) -> Vec<&[CommitInfo]> {
    if batch_size == 0 {
        return vec![commits];
    }
    commits
        .chunks(usize::try_from(batch_size).expect("u32 fits usize"))
        .collect()
}

/// Render one batch of commits as the user prompt (RFC 0005 R3 context).
fn batch_prompt(batch: &[CommitInfo]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("Critique the following commits:\n");
    for commit in batch {
        let _ = write!(
            out,
            "\n=== Commit {sha} ===\nMessage:\n{message}\nChange: {files} file(s), \
             +{ins} -{del}\n",
            sha = commit.short_sha,
            message = commit.message,
            files = commit.stats.files_changed,
            ins = commit.stats.insertions,
            del = commit.stats.deletions,
        );
        if !commit.stats.files.is_empty() {
            let shown = commit.stats.files.iter().take(PROMPT_FILE_CAP);
            let _ = writeln!(
                out,
                "Files: {}",
                shown.cloned().collect::<Vec<_>>().join(", ")
            );
            if commit.stats.files.len() > PROMPT_FILE_CAP {
                let _ = writeln!(
                    out,
                    "(+{} more files)",
                    commit.stats.files.len() - PROMPT_FILE_CAP
                );
            }
        }
        match &commit.patch {
            Some(patch) => {
                let _ = writeln!(out, "Patch excerpt:\n{patch}");
            }
            None => {
                let _ = writeln!(out, "Patch excerpt: (patch content disabled)");
            }
        }
    }
    out
}

/// Pair every commit with its critique, matching echoed shas leniently
/// (full, short, or any prefix relationship).
fn match_critiques(
    commits: &[CommitInfo],
    critiques: &[CommitCritique],
) -> Result<Vec<CommitCritique>, AnalyzeError> {
    let mut matched = Vec::with_capacity(commits.len());
    let mut missing = Vec::new();
    for commit in commits {
        let found = critiques
            .iter()
            .find(|critique| sha_matches(&critique.sha, commit));
        match found {
            Some(critique) => matched.push(critique.clone()),
            None => missing.push(commit.short_sha.clone()),
        }
    }
    if missing.is_empty() {
        Ok(matched)
    } else {
        Err(AnalyzeError::IncompleteCritique {
            expected: commits.len(),
            matched: matched.len(),
            missing: missing.join(", "),
        })
    }
}

/// Whether an echoed sha refers to this commit.
fn sha_matches(echoed: &str, commit: &CommitInfo) -> bool {
    !echoed.is_empty()
        && (commit.sha.starts_with(echoed)
            || echoed.starts_with(&commit.short_sha)
            || commit.short_sha.starts_with(echoed))
}

/// Merge extraction and critique into one report row.
fn to_report_commit(info: &CommitInfo, critique: CommitCritique) -> ReportCommit {
    ReportCommit {
        sha: info.sha.clone(),
        short_sha: info.short_sha.clone(),
        author: format!("{} <{}>", info.author_name, info.author_email),
        date: info.date.clone(),
        message: info.message.clone(),
        files_changed: info.stats.files_changed,
        insertions: info.stats.insertions,
        deletions: info.stats.deletions,
        score: critique.score,
        issue: critique.issue,
        better: critique.better,
        why_good: critique.why_good,
        tags: critique.tags,
        // RFC 0005 R1/R6: one-word detection is plain string logic, local.
        one_word: info.subject.split_whitespace().count() == 1,
    }
}

/// Deterministic aggregates (RFC 0005 R6).
fn compute_stats(commits: &[ReportCommit]) -> Stats {
    let total = commits.len();
    let sum: f64 = commits.iter().map(|c| f64::from(c.score)).sum();
    let count = u32::try_from(total).unwrap_or(u32::MAX);
    let average = if total == 0 {
        0.0
    } else {
        sum / f64::from(count)
    };
    Stats {
        average_score: (average * 10.0).round() / 10.0,
        vague: counted(commits.iter().filter(|c| c.tags.vague).count(), total),
        one_word: counted(commits.iter().filter(|c| c.one_word).count(), total),
    }
}

/// Integer percentage with round-half-up, in pure integer math.
fn counted(count: usize, total: usize) -> Counted {
    let percent = if total == 0 {
        0
    } else {
        (count * 200 + total) / (2 * total)
    };
    Counted { count, percent }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::DiffStat;

    fn commit(short: &str, subject: &str) -> CommitInfo {
        CommitInfo {
            sha: format!("{short}{}", "0".repeat(40 - short.len())),
            short_sha: short.to_owned(),
            author_name: "A".into(),
            author_email: "a@example.com".into(),
            date: "2026-01-01T00:00:00+00:00".into(),
            message: subject.to_owned(),
            subject: subject.to_owned(),
            stats: DiffStat::default(),
            patch: None,
            patch_truncated: false,
        }
    }

    fn critique(sha: &str, score: u8) -> CommitCritique {
        CommitCritique {
            sha: sha.to_owned(),
            score,
            issue: None,
            better: None,
            why_good: None,
            tags: Tags::default(),
        }
    }

    #[test]
    fn chunking_honors_batch_size_and_zero_means_one_batch() {
        let commits: Vec<_> = (0..7).map(|i| commit(&format!("c{i}00000"), "m")).collect();
        assert_eq!(chunk(&commits, 0).len(), 1);
        assert_eq!(chunk(&commits, 3).len(), 3, "7 commits / 3 => 3 batches");
        assert_eq!(chunk(&commits, 10).len(), 1);
    }

    #[test]
    fn critique_matching_accepts_full_and_short_sha_echoes() {
        let commits = vec![commit("abc1234", "one"), commit("def5678", "two")];
        let critiques = vec![
            critique(&commits[1].sha, 9), // full sha echo, out of order
            critique("abc1234", 2),       // short echo
        ];
        let matched = match_critiques(&commits, &critiques).expect("all matched");
        assert_eq!(matched[0].score, 2);
        assert_eq!(matched[1].score, 9);
    }

    #[test]
    fn missing_critiques_are_an_actionable_error() {
        let commits = vec![commit("abc1234", "one"), commit("def5678", "two")];
        let error = match_critiques(&commits, &[critique("abc1234", 3)])
            .expect_err("one commit uncritiqued");
        assert!(error.to_string().contains("def5678"), "got: {error}");
        assert!(error.to_string().contains("1 of 2"), "got: {error}");
    }

    #[test]
    fn stats_are_deterministic_with_rounding() {
        let mut commits = Vec::new();
        for (i, (score, vague, subject)) in [
            (2u8, true, "wip"),
            (9, false, "feat(api): add cache"),
            (5, true, "fix stuff"),
        ]
        .iter()
        .enumerate()
        {
            let mut c = to_report_commit(
                &commit(&format!("c{i}000000"), subject),
                critique("c", *score),
            );
            c.tags.vague = *vague;
            commits.push(c);
        }
        let stats = compute_stats(&commits);
        assert!(
            (stats.average_score - 5.3).abs() < f64::EPSILON,
            "got {}",
            stats.average_score
        );
        assert_eq!(stats.vague.count, 2);
        assert_eq!(stats.vague.percent, 67, "2/3 rounds to 67");
        assert_eq!(stats.one_word.count, 1, "only `wip` is one word");
        assert_eq!(stats.one_word.percent, 33);
    }

    #[test]
    fn prompt_caps_the_file_list_but_not_the_counts() {
        let mut info = commit("abc1234", "touch many files");
        info.stats.files = (0..30).map(|i| format!("file{i}.rs")).collect();
        info.stats.files_changed = 30;
        let prompt = batch_prompt(std::slice::from_ref(&info));
        assert!(prompt.contains("30 file(s)"));
        assert!(prompt.contains("file19.rs"));
        assert!(!prompt.contains("file20.rs"), "capped at {PROMPT_FILE_CAP}");
        assert!(prompt.contains("(+10 more files)"));
    }
}
