//! The probe-rs transport: flash (or attach), poll RTT, decode defmt, route frames.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use defmt_decoder::{DecodeError, Locations, Table};
use probe_rs::flashing::{ElfLoader, ElfOptions, download_file};
use probe_rs::rtt::Rtt;
use probe_rs::{Session, SessionConfig};

use crate::source::{DecodedFrame, SourceEvent, route};

pub struct Config {
    pub elf: PathBuf,
    pub chip: String,
    /// Attach to a target that is already flashed and running, rather than flashing it.
    pub attach_only: bool,
    /// RTT up-channel to read. Defaults to the channel named `defmt`, else channel 0.
    pub channel: Option<usize>,
    pub poll_interval: Duration,
}

pub fn spawn(config: Config, tx: Sender<SourceEvent>) {
    thread::spawn(move || {
        if let Err(error) = run(&config, &tx) {
            let _ = tx.send(SourceEvent::Fatal(format!("{error:#}")));
        }
    });
}

fn run(config: &Config, tx: &Sender<SourceEvent>) -> Result<()> {
    let status = |message: &str| {
        let _ = tx.send(SourceEvent::Status(message.to_string()));
    };
    let stage = |message: &str| {
        let _ = tx.send(SourceEvent::Stage(message.to_string()));
    };

    stage("reading firmware");
    let elf = std::fs::read(&config.elf)
        .with_context(|| format!("reading {}", config.elf.display()))?;
    let table = Table::parse(&elf)
        .context("parsing the defmt table")?
        .ok_or_else(|| anyhow!("{} contains no defmt data", config.elf.display()))?;
    let locations = table.get_locations(&elf).ok().filter(|l| !l.is_empty());

    if !table.has_timestamp() {
        status("no defmt::timestamp! in firmware — graphs use host arrival time");
    }

    stage(&format!("connecting to {}", config.chip));
    let mut session = Session::auto_attach(config.chip.clone(), SessionConfig::default())
        .with_context(|| format!("attaching to {}", config.chip))?;

    if !config.attach_only {
        stage("flashing");
        download_file(&mut session, &config.elf, ElfLoader(ElfOptions::default()))
            .with_context(|| format!("flashing {}", config.elf.display()))?;
        let mut core = session.core(0).context("selecting core 0")?;
        core.reset().context("resetting after flash")?;
    }

    let mut core = session.core(0).context("selecting core 0")?;

    // A previous debug session can leave the core halted — `probe-rs run` interrupted
    // with Ctrl+C does exactly this — in which case the firmware is not executing and
    // RTT would never produce a byte. Resume it rather than waiting on a silent channel.
    if core.core_halted().context("reading core state")? {
        stage("resuming halted core");
        core.run().context("resuming a halted core")?;
    }

    // After a reset the firmware has not necessarily initialised its RTT control block
    // yet, so attaching is retried rather than failed on the first miss.
    stage("waiting for RTT");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut rtt = loop {
        match Rtt::attach(&mut core) {
            Ok(rtt) => break rtt,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(error).context("attaching to RTT (is the firmware running?)");
            }
        }
    };

    let channel = match config.channel {
        Some(requested) => requested,
        None => rtt
            .up_channels()
            .iter()
            .find(|c| c.name() == Some("defmt"))
            .map_or(0, |c| c.number()),
    };
    let up = rtt
        .up_channel(channel)
        .ok_or_else(|| anyhow!("target has no RTT up-channel {channel}"))?;

    let _ = tx.send(SourceEvent::Ready(format!("attached (RTT channel {channel})")));

    let mut stream = table.new_stream_decoder();
    let mut health = DecodeHealth::default();
    let mut buf = [0u8; 4096];
    let attached_at = Instant::now();
    let mut seen_data = false;
    loop {
        let read = up.read(&mut core, &mut buf).context("reading RTT")?;
        if read == 0 {
            // Silence here is ambiguous, and the most common cause is firmware built
            // without DEFMT_LOG, where every frame was compiled away. Say so rather
            // than sitting on a status that claims everything is fine.
            if !seen_data && attached_at.elapsed() > Duration::from_secs(3) {
                status("attached, but target has sent nothing — is DEFMT_LOG set?");
                seen_data = true;
            }
            thread::sleep(config.poll_interval);
            continue;
        }
        if !seen_data {
            seen_data = true;
            let _ = tx.send(SourceEvent::Ready(format!("attached (RTT channel {channel})")));
        }
        stream.received(&buf[..read]);

        loop {
            match stream.decode() {
                Ok(frame) => {
                    health.record_ok();
                    let location = locations.as_ref().and_then(|l| describe(l, frame.index()));
                    let event = route(DecodedFrame {
                        message: frame.display_message().to_string(),
                        timestamp: frame.display_timestamp().map(|t| t.to_string()),
                        level: frame.level().map(|l| format!("{l:?}").to_uppercase()),
                        location,
                    });
                    if tx.send(event).is_err() {
                        // The UI has gone away.
                        return Ok(());
                    }
                }
                Err(DecodeError::UnexpectedEof) => break,
                Err(DecodeError::Malformed) => {
                    // Recoverable: the decoder has already discarded the bad frame and
                    // will resynchronise at the next separator. Only complain once the
                    // failures stop looking like startup noise.
                    if let Some(warning) = health.record_malformed() {
                        status(warning);
                    }
                }
            }
        }
    }
}

/// Distinguishes recoverable decode noise from a genuine firmware/ELF mismatch.
///
/// A malformed frame is not fatal: the rzCOBS decoder discards it and resynchronises at
/// the next separator byte. A few at startup are expected, because `defmt-rtt` keeps its
/// control block in `.uninit` so it survives the reset — the first read can therefore
/// contain the tail of a frame written before the new firmware was flashed. Only a run
/// of failures with nothing successfully decoded points at the wrong ELF.
#[derive(Default)]
struct DecodeHealth {
    decoded: u64,
    malformed: u64,
    warned: bool,
}

impl DecodeHealth {
    /// Failures tolerated before startup noise stops being a plausible explanation.
    const STARTUP_TOLERANCE: u64 = 16;

    fn record_ok(&mut self) {
        self.decoded += 1;
    }

    /// Returns a status message the first time the failures look like a real mismatch.
    fn record_malformed(&mut self) -> Option<&'static str> {
        self.malformed += 1;
        if self.warned || self.decoded > 0 || self.malformed < Self::STARTUP_TOLERANCE {
            return None;
        }
        self.warned = true;
        Some("malformed frames, nothing decoded — does the ELF match what is flashed?")
    }
}

fn describe(locations: &Locations, index: u64) -> Option<String> {
    let location = locations.get(&index)?;
    Some(format!(
        "{}:{}",
        location.file.display(),
        location.line
    ))
}

#[cfg(test)]
mod tests {
    use super::DecodeHealth;

    #[test]
    fn a_few_malformed_frames_at_startup_are_tolerated() {
        let mut health = DecodeHealth::default();
        for _ in 0..DecodeHealth::STARTUP_TOLERANCE - 1 {
            assert_eq!(health.record_malformed(), None);
        }
    }

    #[test]
    fn sustained_failures_with_nothing_decoded_are_reported_once() {
        let mut health = DecodeHealth::default();
        let mut warnings = 0;
        for _ in 0..100 {
            warnings += usize::from(health.record_malformed().is_some());
        }
        assert_eq!(warnings, 1, "the status should be set once, not spammed");
    }

    #[test]
    fn stale_frames_before_a_good_one_never_warn() {
        // The real startup case: leftover bytes from before the reset, then the new
        // firmware's frames decode fine.
        let mut health = DecodeHealth::default();
        assert_eq!(health.record_malformed(), None);
        health.record_ok();
        for _ in 0..100 {
            assert_eq!(health.record_malformed(), None, "occasional noise is not a mismatch");
        }
    }
}
