//! Application state: the topic tree, per-topic sample history, and the log buffer.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use time::{OffsetDateTime, UtcOffset};

/// Which screen is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Monitor,
    Logs,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Monitor, Tab::Logs];

    pub const fn title(self) -> &'static str {
        match self {
            Tab::Monitor => "Monitor",
            Tab::Logs => "Logs",
        }
    }
}

/// A single observed value for a topic.
#[derive(Clone, Debug)]
pub struct Sample {
    /// When the host received it. Always present, but quantised by the RTT poll interval.
    pub host_time: OffsetDateTime,
    /// The firmware's `defmt::timestamp!` value in seconds, when the ELF defines one.
    pub device_time: Option<f64>,
    /// That timestamp as defmt rendered it, for display.
    pub device_time_raw: Option<String>,
    /// The value as defmt rendered it.
    pub value: String,
    /// `value` interpreted as a number, when it is one.
    pub numeric: Option<f64>,
}

/// Derived statistics over a topic's retained history.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub count: usize,
}

/// One monitored topic and its retained history.
#[derive(Debug)]
pub struct Topic {
    pub path: String,
    pub history: VecDeque<Sample>,
    /// Total ever received, which outruns `history.len()` once retention kicks in.
    pub total: u64,
    retention: usize,
}

impl Topic {
    fn new(path: String, retention: usize) -> Self {
        Self {
            path,
            history: VecDeque::new(),
            total: 0,
            retention,
        }
    }

    fn push(&mut self, sample: Sample) {
        self.total += 1;
        if self.history.len() == self.retention {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.history.back()
    }

    /// A topic is graphable when its values parse as numbers. The format spec is fixed
    /// at compile time per topic, so this never flickers between samples.
    pub fn is_graphable(&self) -> bool {
        self.latest().is_some_and(|s| s.numeric.is_some())
    }

    /// Statistics over the retained history. Recomputed per render, which is trivial at
    /// realistic retention limits and avoids drift as samples are evicted.
    pub fn stats(&self) -> Option<Stats> {
        let mut count = 0usize;
        let mut sum = 0.0;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for value in self.history.iter().filter_map(|s| s.numeric) {
            count += 1;
            sum += value;
            min = min.min(value);
            max = max.max(value);
        }
        (count > 0).then(|| Stats {
            min,
            max,
            mean: sum / count as f64,
            count,
        })
    }

    /// Mean interval between retained samples, in seconds.
    ///
    /// Prefers the device clock: RTT is polled in batches, so host arrival times
    /// collapse together and would report a wildly optimistic rate.
    pub fn mean_interval(&self) -> Option<f64> {
        let first = self.history.front()?;
        let last = self.history.back()?;
        let n = self.history.len();
        if n < 2 {
            return None;
        }
        let span = match (first.device_time, last.device_time) {
            (Some(start), Some(end)) => end - start,
            _ => (last.host_time - first.host_time).as_seconds_f64(),
        };
        Some(span / (n - 1) as f64)
    }

    /// Points for the graph, x taken from device time when the firmware provides it.
    ///
    /// Returns `(points, used_device_clock)`. Falling back to host arrival time is
    /// visibly worse — RTT is polled, so bursts collapse onto one x value — hence the
    /// flag, so the UI can say which clock is on the axis.
    pub fn graph_points(&self) -> (Vec<(f64, f64)>, bool) {
        let device = self.history.iter().all(|s| s.device_time.is_some());
        let base_host = self.history.front().map(|s| s.host_time);
        let points = self
            .history
            .iter()
            .filter_map(|s| {
                let y = s.numeric?;
                let x = if device {
                    s.device_time?
                } else {
                    let base = base_host?;
                    (s.host_time - base).as_seconds_f64()
                };
                Some((x, y))
            })
            .collect();
        (points, device)
    }
}

/// A non-monitor defmt frame, shown on the Logs tab.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub host_time: OffsetDateTime,
    pub timestamp: Option<String>,
    pub level: Option<String>,
    pub message: String,
    pub location: Option<String>,
}

/// One phase of bringing the connection up, timed independently.
#[derive(Debug)]
pub struct Stage {
    pub label: String,
    started: Instant,
    /// Set when the stage ends, which freezes its timer.
    finished: Option<Duration>,
    pub failed: bool,
}

impl Stage {
    /// How long the stage took, or how long it has been running.
    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(|| self.started.elapsed())
    }

    pub fn is_done(&self) -> bool {
        self.finished.is_some()
    }
}

/// Progress through the startup sequence, shown centred until it completes.
///
/// Stages accumulate rather than replacing one another, so a failure is readable in the
/// context of everything that already succeeded.
#[derive(Debug, Default)]
pub struct Startup {
    pub stages: Vec<Stage>,
    pub done: bool,
    pub error: Option<String>,
}

impl Startup {
    fn finish_current(&mut self) {
        if let Some(stage) = self.stages.last_mut()
            && stage.finished.is_none()
        {
            stage.finished = Some(stage.started.elapsed());
        }
    }

    /// Begins a stage, completing the previous one.
    pub fn begin(&mut self, label: String) {
        self.finish_current();
        self.stages.push(Stage {
            label,
            started: Instant::now(),
            finished: None,
            failed: false,
        });
    }

    /// Startup succeeded; the normal UI takes over.
    pub fn ready(&mut self) {
        self.finish_current();
        self.done = true;
    }

    /// Startup failed. The stage list stays on screen so the error keeps its context.
    pub fn fail(&mut self, error: String) {
        self.finish_current();
        if let Some(stage) = self.stages.last_mut() {
            stage.failed = true;
        }
        self.error = Some(error);
    }
}

/// Everything the UI renders from.
pub struct App {
    pub startup: Startup,
    pub topics: BTreeMap<String, Topic>,
    pub logs: VecDeque<LogLine>,
    pub tab: Tab,
    pub status: String,
    /// Local UTC offset, resolved once at startup while still single-threaded.
    pub tz: UtcOffset,
    pub retention: usize,
    log_retention: usize,
    /// Set when anything the tree labels display has changed.
    ///
    /// Labels carry live data — each leaf shows its latest value and each branch its
    /// message counts — so this must be set on every sample, not only when a topic is
    /// first seen. The rebuild happens during draw, so it is bounded by the frame rate
    /// rather than by the sample rate.
    pub topics_dirty: bool,
}

impl App {
    pub fn new(tz: UtcOffset, retention: usize, log_retention: usize) -> Self {
        Self {
            startup: Startup::default(),
            topics: BTreeMap::new(),
            logs: VecDeque::new(),
            tab: Tab::Monitor,
            status: String::new(),
            tz,
            retention,
            log_retention,
            topics_dirty: true,
        }
    }

    pub fn push_sample(&mut self, path: &str, sample: Sample) {
        if !self.topics.contains_key(path) {
            self.topics.insert(
                path.to_string(),
                Topic::new(path.to_string(), self.retention),
            );
        }
        // Just inserted above when absent.
        self.topics.get_mut(path).unwrap().push(sample);
        self.topics_dirty = true;
    }

    pub fn push_log(&mut self, line: LogLine) {
        if self.logs.len() == self.log_retention {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    pub fn total_messages(&self) -> u64 {
        self.topics.values().map(|t| t.total).sum()
    }

    /// Topics under a tree path, i.e. those whose segments start with `prefix`.
    pub fn descendants<'a>(&'a self, prefix: &'a [String]) -> impl Iterator<Item = &'a Topic> {
        self.topics.values().filter(move |topic| {
            let mut segments = topic.path.split('/');
            prefix.iter().all(|want| segments.next() == Some(want))
        })
    }
}

/// Interprets a rendered defmt value as a number, so the UI can decide whether to graph
/// it. Booleans map to 0/1 because a state flag is worth plotting as a step.
pub fn parse_numeric(value: &str) -> Option<f64> {
    match value.trim() {
        "true" => Some(1.0),
        "false" => Some(0.0),
        other => other.parse::<f64>().ok(),
    }
}

/// Converts a defmt-rendered timestamp into seconds.
///
/// defmt renders the timestamp either as a bare number (firmware used no display hint)
/// or as `HH:MM:SS.ddd` / `D:HH:MM:SS.ddd` (firmware used `:us`, `:ms` or `:seconds`).
pub fn parse_device_time(raw: &str) -> Option<f64> {
    const MULTIPLIER: [f64; 4] = [1.0, 60.0, 3600.0, 86_400.0];
    let mut total = 0.0;
    for (i, part) in raw.trim().rsplit(':').enumerate() {
        let multiplier = *MULTIPLIER.get(i)?;
        total += part.parse::<f64>().ok()? * multiplier;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_booleans_are_graphable() {
        assert_eq!(parse_numeric("1.023"), Some(1.023));
        assert_eq!(parse_numeric("-7"), Some(-7.0));
        assert_eq!(parse_numeric("true"), Some(1.0));
        assert_eq!(parse_numeric("false"), Some(0.0));
    }

    #[test]
    fn rendered_formats_are_not_graphable() {
        // A derived `Format` enum, which is exactly the case that must not graph.
        assert_eq!(parse_numeric("Charging { mv: 3700 }"), None);
        assert_eq!(parse_numeric("Idle"), None);
        assert_eq!(parse_numeric(""), None);
    }

    #[test]
    fn device_time_handles_both_defmt_renderings() {
        // No display hint: a bare tick count.
        assert_eq!(parse_device_time("12345"), Some(12345.0));
        // `:us` / `:ms` hints render as a clock.
        assert_eq!(parse_device_time("00:00:01.500000"), Some(1.5));
        assert_eq!(parse_device_time("01:02:03.000"), Some(3723.0));
        // With a day component.
        assert_eq!(parse_device_time("2:00:00:01.000"), Some(172_801.0));
        assert_eq!(parse_device_time("nonsense"), None);
    }
}
