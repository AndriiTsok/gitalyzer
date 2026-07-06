//! Gitalyzer binary entry point: parse the CLI, resolve configuration, and
//! dispatch — mapping every outcome to the RFC 0001 R10 exit codes
//! (`0` success, `1` runtime failure, `2` usage error via clap).

use std::env;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use clap::{CommandFactory, Parser};
use gitalyzer::cli::{AnalyzeArgs, Cli, Command, Format, WriteArgs};
use gitalyzer::config::{self, CliOverrides, Settings, Sources};
use gitalyzer::git::{CommitError, Repo, create_commit};
use gitalyzer::provider::AnyProvider;
use gitalyzer::write::WriteSession;
use gitalyzer::{analyze, output};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match run(&cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report(&error);
            ExitCode::from(1)
        }
    }
}

/// Print an error chain to stderr in an actionable, compact form.
fn report(error: &anyhow::Error) {
    eprintln!("error: {error}");
    for cause in error.chain().skip(1) {
        eprintln!("  caused by: {cause}");
    }
}

async fn run(cli: &Cli) -> anyhow::Result<()> {
    // Bare invocation prints help to stdout and exits 0 (RFC 0001 R2).
    let Some(command) = &cli.command else {
        Cli::command().print_help()?;
        return Ok(());
    };

    let sources = Sources::discover(cli.config.as_deref());
    let mut settings = config::load(&sources).context("failed to load configuration")?;
    settings.apply(&cli_overrides(cli, command));
    settings.validate()?;
    tracing::debug!(?settings, "configuration resolved");

    match command {
        Command::Analyze(args) => run_analyze(cli, args, &settings).await,
        Command::Write(args) => run_write(cli, args, &settings).await,
    }
}

/// Execute analysis mode (RFC 0005) and emit the report per RFC 0001 R6/R11.
async fn run_analyze(cli: &Cli, args: &AnalyzeArgs, settings: &Settings) -> anyhow::Result<()> {
    if args.url.is_some() {
        anyhow::bail!(
            "--url (remote analysis) is not implemented yet — landing in slice 6 (RFC 0008)"
        );
    }

    let repo = Repo::discover()?;
    let provider = AnyProvider::from_settings(settings)?;

    // Progress goes to stderr in human mode only; JSON mode is silent
    // (RFC 0007 R1–R2).
    if cli.format == Format::Human {
        eprintln!("Analyzing last {} commits...", settings.analyze.count);
    }

    let report = analyze::run(
        &repo,
        &provider,
        settings,
        args.from.clone(),
        analyze::Repository::local(),
    )
    .await?;

    let rendered = match cli.format {
        Format::Human => output::analysis_human(&report, &settings.analyze.thresholds),
        Format::Json => output::analysis_json(&report),
    };
    emit(cli, &rendered)
}

/// Execute write mode (RFC 0006): suggest a message for the staged changes,
/// then drive the accept/type/regenerate loop — or emit non-interactively in
/// JSON and no-TTY dry-run forms.
async fn run_write(cli: &Cli, args: &WriteArgs, settings: &Settings) -> anyhow::Result<()> {
    let repo = Repo::discover()?;
    let provider = AnyProvider::from_settings(settings)?;
    let session = WriteSession::prepare(&repo, settings)?;

    // Progress on stderr in human mode only (RFC 0007 R1–R2); PRD §4.2 line.
    if cli.format == Format::Human {
        eprintln!(
            "Analyzing staged changes... ({} files changed, +{} -{} lines)",
            session.staged.stats.files_changed,
            session.staged.stats.insertions,
            session.staged.stats.deletions,
        );
    }

    let mut suggestion = session.suggest(&provider, None).await?;

    // JSON mode is always non-interactive and never commits (RFC 0001 R6).
    if cli.format == Format::Json {
        let rendered = output::write_json(&session.report(&suggestion, &provider));
        return emit(cli, &rendered);
    }

    let interactive = stdin_is_interactive();
    if !interactive && !args.dry_run {
        anyhow::bail!(
            "`write` needs an interactive terminal; use --dry-run to print the suggestion \
             or --format json for programmatic use (RFC 0006)"
        );
    }

    // Non-interactive dry run: print the suggestion and stop.
    if !interactive {
        print!("{}", output::suggestion_block(&suggestion));
        return emit_message(cli, &suggestion.message());
    }

    // The accept / type / regenerate loop (RFC 0006 R6, R8).
    loop {
        print!("{}", output::suggestion_block(&suggestion));
        println!();
        print!("Press Enter to accept, type your own message, or 'r' to regenerate:\n> ");
        flush_stdout();

        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            anyhow::bail!("input closed before a choice was made; nothing was committed");
        }
        let input = line.trim_end_matches(['\n', '\r']);

        let message = match input {
            "" => suggestion.message(),
            "r" | "R" => {
                eprintln!("Regenerating...");
                suggestion = session.suggest(&provider, Some(&suggestion)).await?;
                continue;
            }
            typed => typed.to_owned(),
        };

        if args.dry_run {
            return emit_message(cli, &message);
        }

        let workdir = repo
            .workdir()
            .context("cannot commit: the repository has no working tree (bare repository)")?;
        match create_commit(workdir, &message) {
            Ok(outcome) => {
                let subject = message.lines().next().unwrap_or_default();
                println!("✓ Committed {}: \"{subject}\"", outcome.short_sha);
                return Ok(());
            }
            // RFC 0006 R8: show the hook output verbatim, keep the staged
            // state, and return to the prompt — no lost work.
            Err(CommitError::Rejected { output }) => {
                // Re-enter the loop with the same suggestion on display.
                eprintln!("{output}");
                eprintln!("The commit was rejected; adjust the message, or press Ctrl-C to abort.");
            }
            Err(other) => return Err(other.into()),
        }
    }
}

/// Flush the interactive prompt so it appears before the read.
fn flush_stdout() {
    use std::io::Write as _;
    std::io::stdout().flush().ok();
}

/// Print the final dry-run message (RFC 0006 R9), honoring `--output`.
fn emit_message(cli: &Cli, message: &str) -> anyhow::Result<()> {
    let mut rendered = message.to_owned();
    rendered.push('\n');
    emit(cli, &rendered)
}

/// Whether stdin counts as interactive; `GITALYZER_ASSUME_TTY` is the
/// internal testing escape hatch (reserved operational variable).
fn stdin_is_interactive() -> bool {
    if env::var("GITALYZER_ASSUME_TTY").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")) {
        return true;
    }
    std::io::stdin().is_terminal()
}

/// Write the rendered result to stdout or `--output` (RFC 0001 R11,
/// RFC 0005 R9).
fn emit(cli: &Cli, rendered: &str) -> anyhow::Result<()> {
    if let Some(path) = &cli.output {
        write_report(path, rendered)?;
        if cli.format == Format::Human {
            println!("Report written to {}", path.display());
        }
    } else {
        print!("{rendered}");
    }
    Ok(())
}

/// Persist a rendered report to a file with an actionable failure message.
fn write_report(path: &Path, rendered: &str) -> anyhow::Result<()> {
    std::fs::write(path, rendered)
        .with_context(|| format!("cannot write the report to `{}`", path.display()))
}

/// Collect the highest-precedence layer from parsed flags (RFC 0002 R3).
fn cli_overrides(cli: &Cli, command: &Command) -> CliOverrides {
    let mut overrides = CliOverrides {
        provider: cli.provider.clone(),
        model: cli.model.clone(),
        ..CliOverrides::default()
    };
    if let Command::Analyze(args) = command {
        overrides.count = args.count;
        overrides.batch_size = args.batch_size;
    }
    overrides
}

/// Structured logging to stderr (RFC 0007 R5–R6): level from `-v`/`-vv`,
/// overridden by `GITALYZER_LOG`; `GITALYZER_LOG_FORMAT=json` switches to
/// JSON lines.
fn init_tracing(verbosity: u8) {
    let filter = env::var("GITALYZER_LOG")
        .ok()
        .filter(|directives| !directives.is_empty())
        .map_or_else(
            || {
                EnvFilter::new(match verbosity {
                    0 => "warn",
                    1 => "debug",
                    _ => "trace",
                })
            },
            EnvFilter::new,
        );
    let json = env::var("GITALYZER_LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("json"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal());
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}
