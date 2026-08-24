//! A terminal monitor for values published by firmware using `defmt-monitor`.

mod input;
#[cfg(test)]
mod snapshot;
mod model;
mod source;
mod ui;

#[cfg(feature = "probe")]
mod probe;

use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use time::UtcOffset;

use crate::model::App;
use crate::source::SourceEvent;
use crate::ui::UiState;

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
    #[arg(long, default_value_t = 2000)]
    retention: usize,

    /// Log lines retained.
    #[arg(long, default_value_t = 5000)]
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
