//! Report rendering (RFC 0005 R7–R8, RFC 0007).
//!
//! Both renderers consume the same [`AnalysisReport`]; the JSON form is the
//! stable, versioned envelope and the human form follows the PRD layout with
//! configurable score thresholds. Decoration auto-detection (RFC 0007 R3)
//! lands with the polish slice; the human report currently always uses the
//! PRD's decorated layout.

use std::fmt::Write as _;

use crate::analyze::{AnalysisReport, ReportCommit};
use crate::config::Thresholds;

/// Heavy separator line from the PRD mockups.
const SEPARATOR: &str = "━━━━━━━━━━━━━━━━━━━━━━━━━━━━";

/// Render the stable JSON envelope (RFC 0005 R8), pretty-printed, exactly one
/// document (RFC 0007 R2).
pub fn analysis_json(report: &AnalysisReport) -> String {
    let mut rendered =
        serde_json::to_string_pretty(report).expect("report serialization cannot fail");
    rendered.push('\n');
    rendered
}

/// Render the human report (RFC 0005 R5/R7): needs-work worst-first,
/// well-written best-first, the middle band in stats only.
pub fn analysis_human(report: &AnalysisReport, thresholds: &Thresholds) -> String {
    let mut needs_work: Vec<&ReportCommit> = report
        .commits
        .iter()
        .filter(|c| c.score <= thresholds.needs_work)
        .collect();
    needs_work.sort_by_key(|c| c.score);

    let mut well_written: Vec<&ReportCommit> = report
        .commits
        .iter()
        .filter(|c| c.score >= thresholds.well_written)
        .collect();
    well_written.sort_by_key(|c| std::cmp::Reverse(c.score));

    let mut out = String::new();

    if !needs_work.is_empty() {
        section(&mut out, "💩 COMMITS THAT NEED WORK");
        for commit in needs_work {
            let _ = writeln!(out, "Commit: {}", quoted(&commit.message));
            let _ = writeln!(out, "Score: {}/10", commit.score);
            if let Some(issue) = &commit.issue {
                let _ = writeln!(out, "Issue: {issue}");
            }
            if let Some(better) = &commit.better {
                let _ = writeln!(out, "Better: {better}");
            }
            out.push('\n');
        }
    }

    if !well_written.is_empty() {
        section(&mut out, "💎 WELL-WRITTEN COMMITS");
        for commit in well_written {
            let _ = writeln!(out, "Commit: {}", quoted(&commit.message));
            let _ = writeln!(out, "Score: {}/10", commit.score);
            if let Some(why) = &commit.why_good {
                let _ = writeln!(out, "Why it's good: {why}");
            }
            out.push('\n');
        }
    }

    section(&mut out, "📊 YOUR STATS");
    let _ = writeln!(out, "Average score: {:.1}/10", report.stats.average_score);
    let _ = writeln!(
        out,
        "Vague commits: {} ({}%)",
        report.stats.vague.count, report.stats.vague.percent
    );
    let _ = writeln!(
        out,
        "One-word commits: {} ({}%)",
        report.stats.one_word.count, report.stats.one_word.percent
    );
    // RFC 0005 R6: additional tag counts appear only when non-zero.
    let misleading = report.commits.iter().filter(|c| c.tags.misleading).count();
    if misleading > 0 {
        let total = report.commits.len();
        let percent = (misleading * 200 + total) / (2 * total);
        let _ = writeln!(out, "Misleading commits: {misleading} ({percent}%)");
    }

    out
}

/// Append a PRD-style section header.
fn section(out: &mut String, title: &str) {
    let _ = writeln!(out, "{SEPARATOR}");
    let _ = writeln!(out, "{title}");
    let _ = writeln!(out, "{SEPARATOR}");
    out.push('\n');
}

/// Quote a possibly multiline message, indenting continuation lines under
/// the opening quote as in the PRD mockups.
fn quoted(message: &str) -> String {
    let mut lines = message.lines();
    let first = lines.next().unwrap_or_default();
    let mut out = format!("\"{first}");
    for line in lines {
        let _ = write!(out, "\n         {line}");
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_indents_continuation_lines() {
        assert_eq!(quoted("one line"), "\"one line\"");
        assert_eq!(
            quoted("subject\n- detail"),
            "\"subject\n         - detail\""
        );
    }
}
