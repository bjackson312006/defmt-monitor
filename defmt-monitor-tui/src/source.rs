//! Where samples come from: the routing shared by every source, plus a synthetic
//! source used for development and for `--demo`.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use time::OffsetDateTime;

use crate::model::{LogLine, Sample, parse_device_time, parse_numeric};

/// Anything a source can tell the UI.
#[derive(Debug)]
pub enum SourceEvent {
    Sample {
        path: String,
        description: String,
        sample: Sample,
    },
    Log(Box<LogLine>),
    /// A startup phase has begun. The previous one is complete, freezing its timer.
    Stage(String),
    /// Startup succeeded; the UI leaves the loading screen for the normal view.
    Ready(String),
    /// Connection state, shown in the footer once running.
    #[cfg_attr(not(feature = "probe"), allow(dead_code))]
    Status(String),
    /// The source has stopped and will send nothing more.
    ///
    /// Only the probe transport can fail this way; the demo source runs forever.
    #[cfg_attr(not(feature = "probe"), allow(dead_code))]
    Fatal(String),
}

/// A decoded defmt frame, in the form every source can produce.
pub struct DecodedFrame {
    /// `Frame::display_message()`, i.e. the format string with arguments substituted.
    pub message: String,
    /// `Frame::display_timestamp()`, absent when the firmware defines no `timestamp!`.
    pub timestamp: Option<String>,
    pub level: Option<String>,
    pub location: Option<String>,
}

/// Routes one decoded frame to either a monitor sample or a log line.
///
/// This is the whole of the monitor/log split, and it is deliberately the only place
/// that decision is made, so both sources behave identically.
pub fn route(frame: DecodedFrame) -> SourceEvent {
    let host_time = OffsetDateTime::now_utc();

    if let Some((path, description, value)) = defmt_monitor::parse_frame(&frame.message) {
        let device_time = frame.timestamp.as_deref().and_then(parse_device_time);
        return SourceEvent::Sample {
            path: path.to_string(),
            description: description.to_string(),
            sample: Sample {
                host_time,
                device_time,
                device_time_raw: frame.timestamp,
                numeric: parse_numeric(value),
                value: value.to_string(),
            },
        };
    }

    // A frame from firmware built against an older wire format would otherwise be
    // listed as ordinary log output, leaving no clue why its topic never appears.
    let message = if defmt_monitor::is_legacy_frame(&frame.message) {
        format!(
            "{} <- older defmt-monitor wire format; rebuild the firmware",
            frame.message
        )
    } else {
        frame.message
    };

    SourceEvent::Log(Box::new(LogLine {
        host_time,
        timestamp: frame.timestamp,
        level: frame.level,
        message,
        location: frame.location,
    }))
}

/// Synthetic data covering every case the UI has to handle: numeric topics at different
/// rates, a non-graphable enum topic, a boolean, and interleaved log lines.
pub fn spawn_demo(tx: Sender<SourceEvent>) {
    thread::spawn(move || {
        // A brief stage so `--demo` exercises the loading screen too.
        let _ = tx.send(SourceEvent::Stage("preparing demo data".to_string()));
        thread::sleep(Duration::from_millis(400));
        let _ = tx.send(SourceEvent::Ready("demo (no probe attached)".to_string()));

        // A tiny LCG keeps the demo deterministic without pulling in a rand dependency.
        let mut rng: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((rng >> 33) as f64) / (u32::MAX as f64)
        };

        let mut tick: u64 = 0;
        let mut battery = 3700.0_f64;

        loop {
            // Device timestamp in defmt's `:ms` rendering, so the device-clock path is
            // exercised rather than the host-time fallback.
            let ms = tick * 20;
            let stamp = format!(
                "{:02}:{:02}:{:02}.{:03}",
                ms / 3_600_000,
                (ms / 60_000) % 60,
                (ms / 1000) % 60,
                ms % 1000
            );

            let t = tick as f64 * 0.02;
            let emit = |message: String| {
                let _ = tx.send(route(DecodedFrame {
                    message,
                    timestamp: Some(stamp.clone()),
                    level: Some("INFO".to_string()),
                    location: Some("src/main.rs:42".to_string()),
                }));
            };

            emit(format!("[MON2][imu/accel/x][lateral g][{:.3}]", (t * 1.7).sin() * 2.0));
            emit(format!("[MON2][imu/accel/y][longitudinal g][{:.3}]", (t * 0.9).cos() * 1.5));
            emit(format!("[MON2][imu/accel/z][vertical g][{:.3}]", 9.81 + next() * 0.1));

            if tick.is_multiple_of(5) {
                battery += (next() - 0.5) * 8.0;
                battery = battery.clamp(3200.0, 4200.0);
                emit(format!("[MON2][power/battery_mv][pack voltage][{}]", battery as u32));
                // Not graphable: a derived `Format` enum renders as text.
                let state = match (tick / 100) % 3 {
                    0 => "Idle".to_string(),
                    1 => format!("Charging {{ mv: {} }}", battery as u32),
                    _ => "Fault(3)".to_string(),
                };
                emit(format!("[MON2][power/state][charger state machine][{state}]"));
                emit(format!("[MON2][sys/ready][][{}]", tick % 200 < 100));
            }
            if tick.is_multiple_of(25) {
                emit(format!("[MON2][sys/uptime_ms][since boot][{ms}]"));
            }

            // Ordinary log traffic, which must land on the Logs tab rather than the tree.
            if tick.is_multiple_of(40) {
                let (level, message) = match (tick / 40) % 4 {
                    0 => ("INFO", "sensor poll complete".to_string()),
                    1 => ("DEBUG", format!("i2c transaction {} ok", tick / 40)),
                    2 => ("WARN", "i2c retry".to_string()),
                    _ => ("ERROR", "imu saturated".to_string()),
                };
                let _ = tx.send(route(DecodedFrame {
                    message,
                    timestamp: Some(stamp.clone()),
                    level: Some(level.to_string()),
                    location: Some("src/sensor.rs:118".to_string()),
                }));
            }

            tick += 1;
            thread::sleep(Duration::from_millis(20));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(message: &str) -> DecodedFrame {
        DecodedFrame {
            message: message.to_string(),
            timestamp: Some("00:00:01.500".to_string()),
            level: Some("INFO".to_string()),
            location: None,
        }
    }

    #[test]
    fn monitor_frames_become_samples() {
        match route(frame("[MON2][imu/accel/x][lateral g][1.023]")) {
            SourceEvent::Sample {
                path,
                description,
                sample,
            } => {
                assert_eq!(path, "imu/accel/x");
                assert_eq!(description, "lateral g");
                assert_eq!(sample.value, "1.023");
                assert_eq!(sample.numeric, Some(1.023));
                assert_eq!(sample.device_time, Some(1.5));
            }
            other => panic!("expected a sample, got {other:?}"),
        }
    }

    #[test]
    fn non_numeric_samples_keep_their_rendering() {
        match route(frame("[MON2][power/state][][Charging { mv: 3700 }]")) {
            SourceEvent::Sample { path, sample, .. } => {
                assert_eq!(path, "power/state");
                assert_eq!(sample.value, "Charging { mv: 3700 }");
                assert_eq!(sample.numeric, None);
            }
            other => panic!("expected a sample, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_frames_become_logs() {
        match route(frame("spawning task 3")) {
            SourceEvent::Log(line) => {
                assert_eq!(line.message, "spawning task 3");
                assert_eq!(line.level.as_deref(), Some("INFO"));
            }
            other => panic!("expected a log line, got {other:?}"),
        }
    }
}
