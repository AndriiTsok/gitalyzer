//! Gitalyzer binary entry point: parse the CLI, resolve configuration, and
//! dispatch — mapping every outcome to the RFC 0001 R10 exit codes
//! (`0` success, `1` runtime failure, `2` usage error via clap).

use std::env;
use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::Context;
use clap::{CommandFactory, Parser};
use gitalyzer::cli::{Cli, Command};
use gitalyzer::config::{self, CliOverrides, Sources};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match run(&cli) {
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

fn run(cli: &Cli) -> anyhow::Result<()> {
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
        Command::Analyze(_) => {
            anyhow::bail!("`analyze` is not implemented yet — landing in slice 4 (RFC 0008)")
        }
        Command::Write(_) => {
            anyhow::bail!("`write` is not implemented yet — landing in slice 5 (RFC 0008)")
        }
    }
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
