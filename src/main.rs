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
use gitalyzer::git::{
    CommitError, RemoteClone, Repo, clone_for_analysis, create_commit, interrupt_clones,
};
use gitalyzer::output::Progress;
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
    let provider = AnyProvider::from_settings(settings)?;
    // Progress on stderr in human mode; silent in JSON mode (RFC 0007 R1–R2).
    let progress = Progress::stderr(cli.format == Format::Human);

    let (holder, repository) = acquire_repository(args, settings, &progress).await?;

    if cli.format == Format::Human {
        eprintln!("Analyzing last {} commits...", settings.analyze.count);
    }

    let on_batch = |done: usize, total: usize| {
        progress.step(format!("Critiquing batch {done}/{total}..."));
    };
    // Ctrl-C aborts cleanly: temporary clones drop with the cancelled future
    // (RFC 0004 R5, RFC 0007 R9).
    let report = tokio::select! {
        result = analyze::run(
            holder.repo(),
            &provider,
            settings,
            args.from.clone(),
            repository,
            on_batch,
        ) => result?,
        _ = tokio::signal::ctrl_c() => {
            interrupt_clones();
            anyhow::bail!("interrupted — no report was produced");
        }
    };
    progress.finish();

    let rendered = match cli.format {
        Format::Human => {
            output::analysis_human(&report, &settings.analyze.thresholds, stdout_decorated(cli))
        }
        Format::Json => output::analysis_json(&report),
    };
    emit(cli, &rendered)
}

/// The repository under analysis: discovered locally, or a temporary remote
/// clone that lives as long as this value (RFC 0004 R5).
enum RepoHolder {
    Local(Repo),
    Remote(RemoteClone),
}

impl RepoHolder {
    fn repo(&self) -> &Repo {
        match self {
            Self::Local(repo) => repo,
            Self::Remote(clone) => &clone.repo,
        }
    }
}

/// Open the local repository, or clone the `--url` remote (RFC 0004 R5):
/// shallow at `count + buffer` normally, full when `--from` must resolve
/// arbitrary history, and re-cloned full when the shallow boundary proves
/// too tight for the requested range.
async fn acquire_repository(
    args: &AnalyzeArgs,
    settings: &Settings,
    progress: &Progress,
) -> anyhow::Result<(RepoHolder, analyze::Repository)> {
    let Some(url) = &args.url else {
        return Ok((
            RepoHolder::Local(Repo::discover()?),
            analyze::Repository::local(),
        ));
    };

    progress.step(format!("Cloning {url}..."));
    let depth = if args.from.is_some() {
        None
    } else {
        Some(settings.analyze.count)
    };
    let clone = clone_blocking(url.clone(), args.branch.clone(), depth).await?;

    let clone = if clone.shallow && range_exceeds_clone(&clone, settings, args) {
        progress
            .step("Shallow history is too small for the requested range; fetching full history...");
        clone_blocking(url.clone(), args.branch.clone(), None).await?
    } else {
        clone
    };

    Ok((
        RepoHolder::Remote(clone),
        analyze::Repository::remote(url.clone()),
    ))
}

/// Run the blocking gix clone off the async runtime, aborting promptly on
/// Ctrl-C via the shared interrupt flag.
async fn clone_blocking(
    url: String,
    branch: Option<String>,
    depth: Option<u32>,
) -> anyhow::Result<RemoteClone> {
    let handle =
        tokio::task::spawn_blocking(move || clone_for_analysis(&url, branch.as_deref(), depth));
    tokio::select! {
        joined = handle => Ok(joined.context("clone task panicked")??),
        _ = tokio::signal::ctrl_c() => {
            interrupt_clones();
            anyhow::bail!("interrupted while cloning — the temporary clone is discarded");
        }
    }
}

/// Cheap probe (no patch content) checking whether the walk can satisfy the
/// requested count within the shallow boundary (RFC 0004 R5).
fn range_exceeds_clone(clone: &RemoteClone, settings: &Settings, args: &AnalyzeArgs) -> bool {
    let requested = usize::try_from(settings.analyze.count).expect("u32 fits usize");
    let probe = clone.repo.history(&gitalyzer::git::HistoryOptions {
        from: args.from.clone(),
        count: requested,
        max_patch_bytes: 0,
    });
    match probe {
        Ok(commits) => commits.len() < requested,
        // Let the real run surface empty-history and revision errors.
        Err(_) => false,
    }
}

/// Whether stdout output should carry decoration (RFC 0007 R3, RFC 0001
/// R12): never under `--no-color` or a non-empty `NO_COLOR`, never into
/// `--output` files, otherwise when stdout is a terminal.
fn stdout_decorated(cli: &Cli) -> bool {
    if cli.no_color || env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    if cli.output.is_some() {
        return false;
    }
    if env::var("GITALYZER_ASSUME_TTY").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")) {
        return true;
    }
    std::io::stdout().is_terminal()
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

    let decorated = stdout_decorated(cli);

    // Non-interactive dry run: print the suggestion and stop.
    if !interactive {
        print!("{}", output::suggestion_block(&suggestion, decorated));
        return emit_message(cli, &suggestion.message());
    }

    // The accept / type / regenerate loop (RFC 0006 R6, R8).
    loop {
        print!("{}", output::suggestion_block(&suggestion, decorated));
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
                let mark = if decorated { "✓ " } else { "" };
                println!("{mark}Committed {}: \"{subject}\"", outcome.short_sha);
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
