//! Command-line surface (RFC 0001).
//!
//! Only parsing lives here; defaults for values that participate in the
//! configuration precedence chain (RFC 0002 R3) are deliberately *not* set via
//! clap — a flag left unset must be distinguishable from an explicitly passed
//! value, so built-in defaults are applied by [`crate::config`] instead.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Usage examples shown at the bottom of `--help` (RFC 0001 canonical shape).
const EXAMPLES: &str = "\
Examples:
  gitalyzer analyze
  gitalyzer analyze --url=\"https://github.com/example/project\"
  gitalyzer analyze --url=\"git@github.com:example/project.git\" --branch develop
  gitalyzer analyze -n 100 --from 1a2b3c4
  gitalyzer analyze --format json --output report.json
  gitalyzer write
  gitalyzer write --dry-run";

/// AI-powered critique of Git commit messages, and help writing better ones.
#[derive(Debug, Parser)]
#[command(name = "gitalyzer", version, about, after_help = EXAMPLES)]
pub struct Cli {
    /// The mode to run. Omitted entirely → help is printed and the process
    /// exits `0` (RFC 0001 R2); that fallback is handled in `main`.
    #[command(subcommand)]
    pub command: Option<Command>,

    // Doc comments below double as `--help` text (clap), so they stay in
    // product voice; the RFC mapping is: provider/model 0001 R7, format R6,
    // output R11, config R8, no_color R12, verbose R13.
    /// Override the configured LLM provider
    #[arg(long, global = true, value_name = "ID")]
    pub provider: Option<String>,

    /// Override the configured model
    #[arg(long, global = true, value_name = "NAME")]
    pub model: Option<String>,

    /// Output format
    #[arg(long, global = true, value_enum, default_value = "human")]
    pub format: Format,

    /// Write the result to a file instead of stdout
    #[arg(long, global = true, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Use an explicit configuration file instead of discovery
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Force undecorated output (the NO_COLOR variable is honored too)
    #[expect(
        clippy::doc_markdown,
        reason = "doc comment doubles as clap help text, where backticks render literally"
    )]
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Increase diagnostic verbosity (-v debug, -vv trace)
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

/// Output format selected by `--format` (RFC 0001 R6; JSON cleanliness is
/// RFC 0007 R2). Variant docs render in `--help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Rich terminal report for humans
    Human,
    /// Exactly one machine-parseable JSON document on stdout
    Json,
}

/// The two Gitalyzer modes (RFC 0001 R1).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Critique recent commit messages of a repository
    Analyze(AnalyzeArgs),
    /// Suggest a commit message for the staged changes
    Write(WriteArgs),
}

/// History selection and batching for `analyze` (RFC 0001 R3–R4).
#[derive(Debug, clap::Args)]
pub struct AnalyzeArgs {
    /// Analyze a remote repository instead of the current one; accepts any Git
    /// transport (https, ssh, scp-style, git, file)
    #[arg(long, value_name = "GIT_URL")]
    pub url: Option<String>,

    /// Branch to analyze; only valid together with --url [default: the
    /// remote's default branch]
    #[arg(long, value_name = "NAME", requires = "url")]
    pub branch: Option<String>,

    /// Number of commits to analyze [default: 50]
    #[arg(short = 'n', long, value_name = "N")]
    pub count: Option<u32>,

    /// Start walking history from this revision instead of HEAD (SHA, branch,
    /// tag, HEAD~20, ...)
    #[arg(long, value_name = "REVISION")]
    pub from: Option<String>,

    /// Commits per LLM request; 0 sends everything in a single request
    /// [default: 10]
    #[arg(long, value_name = "N")]
    pub batch_size: Option<u32>,
}

/// Options for `write` (RFC 0001 R5).
#[derive(Debug, clap::Args)]
pub struct WriteArgs {
    /// Run the full flow but print the final message instead of committing
    #[arg(long)]
    pub dry_run: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn analyze_flags_parse() {
        let cli = Cli::parse_from([
            "gitalyzer",
            "analyze",
            "--url",
            "git@example.com:a/b.git",
            "--branch",
            "dev",
            "-n",
            "100",
            "--from",
            "abc123",
            "--batch-size",
            "0",
        ]);
        let Some(Command::Analyze(args)) = cli.command else {
            panic!("expected analyze subcommand");
        };
        assert_eq!(args.url.as_deref(), Some("git@example.com:a/b.git"));
        assert_eq!(args.branch.as_deref(), Some("dev"));
        assert_eq!(args.count, Some(100));
        assert_eq!(args.from.as_deref(), Some("abc123"));
        assert_eq!(args.batch_size, Some(0));
    }

    #[test]
    fn global_flags_work_after_subcommand() {
        let cli = Cli::parse_from(["gitalyzer", "analyze", "--format", "json", "-vv"]);
        assert_eq!(cli.format, Format::Json);
        assert_eq!(cli.verbose, 2);
    }
}
