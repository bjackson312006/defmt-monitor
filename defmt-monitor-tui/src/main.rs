//! A terminal monitor for values published by firmware using `defmt-monitor`.

mod input;
#[cfg(test)]
mod snapshot;
mod model;
mod source;
mod ui;

#[cfg(feature = "probe")]
mod probe;

use std::io::{Write, stdout};
use std::process::{Command, Stdio};
use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use time::UtcOffset;

use crate::model::App;
use crate::source::SourceEvent;
use crate::ui::UiState;

/// Default value for sample retention (how many samples are retained per topic for the TUI history)
pub const SAMPLE_RETENTION_DEFAULT: usize = 2000;
/// Default value for log line retention (how many log lines are retained for the TUI history)
pub const LOG_LINE_RETENTION_DEFAULT: usize = 5000;

/// Terminal monitor for firmware values published with `defmt-monitor`.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Firmware ELF. Required unless --demo; the decoder needs it to resolve frames.
    elf: Option<PathBuf>,

    /// Target chip name, e.g. STM32F411CEUx.
    #[arg(long)]
    chip: Option<String>,

    /// Attach to an already-running target instead of flashing it first.
    #[arg(long)]
    attach: bool,

    /// RTT up-channel to read. Defaults to the channel named `defmt`, else 0.
    #[arg(long)]
    channel: Option<usize>,

    /// Samples retained per topic.
    #[arg(long, default_value_t = SAMPLE_RETENTION_DEFAULT)]
    retention: usize,

    /// Log lines retained.
    #[arg(long, default_value_t = LOG_LINE_RETENTION_DEFAULT)]
    log_retention: usize,

    /// Run against synthetic data with no probe attached.
    #[arg(long)]
    demo: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Resolved before any thread is spawned: `time` refuses to read the local offset
    // from a multi-threaded process, and the source threads start below.
    let tz = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

    let (tx, rx) = mpsc::channel();
    if cli.demo {
        source::spawn_demo(tx);
    } else {
        spawn_probe(&cli, tx)?;
    }

    let mut app = App::new(tz, cli.retention, cli.log_retention);
    let mut ui = UiState::default();

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture).context("enabling mouse capture")?;
    let result = run(&mut terminal, &mut app, &mut ui, &rx);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

#[cfg(feature = "probe")]
fn spawn_probe(cli: &Cli, tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    let elf = cli
        .elf
        .clone()
        .context("an ELF path is required (or pass --demo)")?;
    let chip = cli
        .chip
        .clone()
        .context("--chip is required (or pass --demo)")?;
    probe::spawn(
        probe::Config {
            elf,
            chip,
            attach_only: cli.attach,
            channel: cli.channel,
            poll_interval: Duration::from_millis(10),
        },
        tx,
    );
    Ok(())
}

#[cfg(not(feature = "probe"))]
fn spawn_probe(_cli: &Cli, _tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    anyhow::bail!("built without the `probe` feature; only --demo is available")
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    ui: &mut UiState,
    rx: &mpsc::Receiver<SourceEvent>,
) -> Result<()> {
    // Done once only: after this the selection belongs to the user, and re-selecting
    // under them would fight their navigation.
    let mut bootstrapped = false;

    loop {
        if let Some(text) = ui.pager.take() {
            show_in_pager(terminal, &text)?;
        }

        drain(rx, app);

        if !bootstrapped {
            bootstrapped = ui.select_first_topic(app);
        }

        terminal.draw(|frame| ui::draw(frame, app, ui))?;

        if event::poll(Duration::from_millis(16))? && input::handle(event::read()?, app, ui) {
            return Ok(());
        }
    }
}

/// Leaves the TUI, shows `text` in the user's pager, and comes back.
///
/// The same terminal is reused rather than spawning a new one: there is no portable way
/// to launch a terminal emulator, and it would fail over SSH. Leaving the alternate
/// screen is what matters — the pager does not continuously redraw, so the terminal's own
/// selection and copy behave normally.
fn show_in_pager(terminal: &mut ratatui::DefaultTerminal, text: &str) -> Result<()> {
    // Step out of the TUI in place. Replacing the terminal with a fresh `ratatui::init()`
    // looks equivalent but is not: the old value is dropped *after* the new one is built,
    // and its teardown then undoes the setup that just happened.
    let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen);
    disable_raw_mode().context("leaving raw mode")?;

    if let Err(error) = pipe_to_pager(text) {
        // No pager available: plain output is still selectable, which is the whole point.
        println!("{text}");
        println!("({error}; press Enter to return)");
        let _ = std::io::stdin().read_line(&mut String::new());
    }

    enable_raw_mode().context("re-entering raw mode")?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)
        .context("returning to the alternate screen")?;

    // Deliberately no `terminal.clear()`. The alternate screen keeps its contents while
    // the pager runs on the normal screen, so ratatui's cached buffer still matches what
    // is displayed and the next draw repaints only what changed. `clear()` would also
    // round-trip a cursor-position query to the terminal and fail the whole session if
    // no reply arrived.
    let _ = terminal;
    Ok(())
}

fn pipe_to_pager(text: &str) -> Result<()> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let mut child = Command::new(&pager)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {pager}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        // A pager quit early closes the pipe; that is not an error worth reporting.
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().context("waiting for the pager")?;
    Ok(())
}

/// Moves pending source events into the model.
///
/// Bounded per tick so that a fast target cannot starve rendering and input.
fn drain(rx: &mpsc::Receiver<SourceEvent>, app: &mut App) {
    for _ in 0..10_000 {
        match rx.try_recv() {
            Ok(SourceEvent::Sample { path, sample }) => app.push_sample(&path, sample),
            Ok(SourceEvent::Log(line)) => app.push_log(*line),
            Ok(SourceEvent::Stage(label)) => app.startup.begin(label),
            Ok(SourceEvent::Ready(status)) => {
                app.startup.ready();
                app.status = status;
            }
            Ok(SourceEvent::Status(status)) => app.status = status,
            // Before startup completes the error belongs on the loading screen, where
            // there is room to read it; afterwards the footer is the only place left.
            Ok(SourceEvent::Fatal(error)) => {
                if app.startup.done {
                    app.status = format!("stopped: {error}");
                } else {
                    app.startup.fail(error);
                }
            }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                if !app.status.starts_with("stopped") {
                    app.status = "source disconnected".to_string();
                }
                return;
            }
        }
    }
}
