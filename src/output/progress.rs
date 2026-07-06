//! Progress indication (RFC 0007 R1–R2): a rich spinner on stderr when it is
//! a terminal, plain text lines otherwise (CI, pipes), and complete silence
//! in JSON mode.

use std::borrow::Cow;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

/// Progress reporter with automatic degradation.
pub struct Progress {
    mode: Mode,
}

enum Mode {
    /// Animated spinner with step messages (stderr is a TTY).
    Rich(ProgressBar),
    /// One plain stderr line per step (pipes, CI).
    Plain,
    /// Nothing at all (JSON mode, RFC 0007 R2).
    Silent,
}

impl Progress {
    /// Build the right reporter: `active` is false in JSON mode; rich output
    /// additionally requires stderr to be a terminal.
    pub fn stderr(active: bool) -> Self {
        use std::io::IsTerminal as _;
        let mode = if !active {
            Mode::Silent
        } else if std::io::stderr().is_terminal() {
            let bar = ProgressBar::new_spinner();
            bar.set_style(
                ProgressStyle::with_template("{spinner} {msg}").expect("static template is valid"),
            );
            bar.enable_steady_tick(Duration::from_millis(120));
            Mode::Rich(bar)
        } else {
            Mode::Plain
        };
        Self { mode }
    }

    /// Announce the current step.
    pub fn step(&self, message: impl Into<Cow<'static, str>>) {
        let message = message.into();
        match &self.mode {
            Mode::Rich(bar) => bar.set_message(message),
            Mode::Plain => eprintln!("{message}"),
            Mode::Silent => {}
        }
    }

    /// Clear the indicator so the report starts on a clean line.
    pub fn finish(&self) {
        if let Mode::Rich(bar) = &self.mode {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
    }
}
