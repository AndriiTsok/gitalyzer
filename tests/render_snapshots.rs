//! Snapshot tests (RFC 0007 R11) pinning both renderers against a fixed,
//! hand-built report — the exact bytes of the human layout and the JSON
//! envelope are part of the product contract (RFC 0005 R7–R8).

use gitalyzer::analyze::{
    AnalysisReport, Counted, Meta, Range, ReportCommit, Repository, Stats, Tags,
};
use gitalyzer::config::Thresholds;
use gitalyzer::output;

/// A deterministic report exercising all three buckets.
fn fixed_report() -> AnalysisReport {
    let commit = |short: &str, message: &str, score: u8| ReportCommit {
        sha: format!("{short}{}", "0".repeat(40 - short.len())),
        short_sha: short.to_owned(),
        author: "Test Author <test@example.com>".to_owned(),
        date: "2026-01-01T00:00:00+00:00".to_owned(),
        message: message.to_owned(),
        files_changed: 2,
        insertions: 10,
        deletions: 3,
        score,
        issue: None,
        better: None,
        why_good: None,
        tags: Tags::default(),
        one_word: !message.contains(' '),
    };

    let mut wip = commit("aaaa111", "wip", 1);
    wip.issue = Some("No information about what's in progress".to_owned());
    wip.better = Some("Describe what you're working on".to_owned());
    wip.tags = Tags {
        vague: true,
        misleading: false,
        no_why: true,
    };

    let mut fix = commit("bbbb222", "fixed bug", 2);
    fix.issue = Some("Too vague - which bug? What was the impact?".to_owned());
    fix.better = Some("fix(auth): resolve token expiration handling".to_owned());
    fix.tags = Tags {
        vague: true,
        misleading: false,
        no_why: true,
    };

    let middle = commit("cccc333", "update parser tests", 6);

    let mut good = commit(
        "dddd444",
        "feat(api): add Redis caching layer\n- Implement cache for read endpoints\n- Add TTL configuration",
        9,
    );
    good.why_good = Some("Clear scope, specific changes, measurable impact".to_owned());

    AnalysisReport {
        schema_version: 1,
        mode: "analyze",
        repository: Repository::local(),
        range: Range {
            from: "HEAD".to_owned(),
            requested: 50,
            analyzed: 4,
        },
        commits: vec![good, middle, fix, wip],
        stats: Stats {
            average_score: 4.5,
            vague: Counted {
                count: 2,
                percent: 50,
            },
            one_word: Counted {
                count: 1,
                percent: 25,
            },
        },
        meta: Meta {
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-5".to_owned(),
            batches: 1,
        },
    }
}

#[test]
fn human_report_layout_is_stable() {
    let rendered = output::analysis_human(&fixed_report(), &Thresholds::default());
    insta::assert_snapshot!("analysis_human", rendered);
}

#[test]
fn json_envelope_is_stable() {
    let rendered = output::analysis_json(&fixed_report());
    insta::assert_snapshot!("analysis_json", rendered);
}
