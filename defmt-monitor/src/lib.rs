//! Publish named values from embedded firmware over [`defmt`], for display in a
//! host-side monitor TUI.
//!
//! [`defmt`]: https://docs.rs/defmt
//!
//! # Usage
//!
//! ```ignore
//! defmt_monitor::monitor!("imu/accel/x", "{=f32}", accel.x);
//! defmt_monitor::monitor!("imu/accel/y", "{=f32}", accel.y);
//! defmt_monitor::monitor!("power/battery_mv", "{=u16}", battery_mv);
//! ```
//!
//! Each call expands to a single `defmt::println!`, so monitor frames travel over
//! whatever transport the application already configured (typically `defmt-rtt`) and
//! require no extra setup. Set `DEFMT_MONITOR=off` to compile them out entirely.
//!
//! Your crate must depend on `defmt` directly: `defmt`'s macros expand to bare
//! `defmt::export::*` paths that resolve against the calling crate. In exchange,
//! `defmt-monitor` itself has no `defmt` dependency and works with any defmt version.
//!
//! # Wire format
//!
//! `monitor!` folds the topic into the *format string* rather than passing it as an
//! argument. defmt interns format strings into the `.defmt` section of the ELF at
//! compile time and transmits only an index, so the topic costs zero bytes at runtime
//! no matter how long it is. `monitor!("imu/accel/x", "{=f32}", v)` sends a 2-byte
//! format index plus 4 bytes of payload.
//!
//! The interned string has the shape:
//!
//! ```text
//! [MON1][<topic>][<value format spec>]
//! ```
//!
//! A host decodes frames normally (it needs the ELF either way) and routes them by
//! testing each frame with [`parse_frame`]. Because the sentinel and topic live in the
//! interned format string, the match is unaffected by the application's `defmt.toml`,
//! `--log-format`, timestamp, or any other host-side display configuration.
//!
//! # Turning it off
//!
//! `monitor!` expands to `defmt::println!` rather than `defmt::info!`. Monitor samples
//! are not a log level, and `println!` carries no level tag, so `DEFMT_LOG` cannot
//! filter them away — firmware built with `DEFMT_LOG` unset still publishes. It shares
//! `info!`'s codegen in every other respect, timestamp included.
//!
//! To compile the calls out entirely, set `DEFMT_MONITOR` to `off` (or `0`, `false`,
//! `no`). Unset means enabled, so adding a `monitor!` call to a fresh project produces
//! data without first having to discover this variable.
//!
//! When disabled nothing is emitted and no topic is interned, so a production build
//! carries no trace of the monitor and a stock `probe-rs run` console stays clean:
//!
//! ```toml
//! # .cargo/config.toml
//! [env]
//! DEFMT_MONITOR = "off"
//! ```
//!
//! Arguments are still name-resolved and type-checked when disabled, so they cannot rot
//! unnoticed, but they are not checked against the format spec — that requires a build
//! with monitoring on.
//!
//! # Caveats
//!
//! Monitor frames share one RTT ring buffer with the application's ordinary logs. At
//! high sample rates they will start crowding out log messages, since `defmt-rtt` drops
//! on overflow. Raising `DEFMT_RTT_BUFFER_SIZE` is the cheap fix.

#![cfg_attr(not(test), no_std)]

/// Publishes a named value as a monitor frame.
///
/// Takes a topic literal, a [defmt format string] literal for the value, and the
/// arguments it consumes:
///
/// ```ignore
/// monitor!("imu/accel/x", "{=f32}", accel.x);
/// monitor!("net/endpoint", "{=u8}.{=u8}.{=u8}.{=u8}", a, b, c, d);
/// ```
///
/// Topics are conventionally `/`-separated, which the TUI renders as a tree. A topic
/// may not contain `[`, `]`, `{` or `}`.
///
/// [defmt format string]: https://defmt.ferrous-systems.com/macros
pub use defmt_monitor_macros::monitor;

/// Prefix identifying a monitor frame, and the version of the wire format.
///
/// Bump this if the shape of the interned string ever changes, so that a host can
/// recognise — and refuse — frames from firmware it does not understand.
pub const SENTINEL: &str = "[MON1]";

/// Splits a monitor frame into its topic and payload, or returns [`None`] if it is not
/// a monitor frame. This is the host side's entire routing decision.
///
/// The same function serves both inputs a host has available, because `monitor!` gives
/// the format string and the rendered message an identical shape:
///
/// - a frame's **format string**, where the payload is the value's format spec
/// - a frame's **rendered message**, where the payload is the formatted value
///
/// `defmt_decoder::Frame` keeps its format string private, so in practice a host calls
/// this on `frame.display_message().to_string()`.
///
/// ```
/// use defmt_monitor::parse_frame;
///
/// // Format string: payload is the spec.
/// assert_eq!(parse_frame("[MON1][imu/accel/x][{=f32}]"), Some(("imu/accel/x", "{=f32}")));
/// // Rendered message: payload is the value.
/// assert_eq!(parse_frame("[MON1][imu/accel/x][1.023]"), Some(("imu/accel/x", "1.023")));
/// // Not a monitor frame.
/// assert_eq!(parse_frame("spawning task {=str}"), None);
/// ```
pub fn parse_frame(frame: &str) -> Option<(&str, &str)> {
    let rest = frame.strip_prefix(SENTINEL)?.strip_prefix('[')?;
    // A topic cannot contain `]`, so the first `][` is always the delimiter, even when
    // the payload itself contains brackets.
    let (topic, rest) = rest.split_once("][")?;
    let payload = rest.strip_suffix(']')?;
    Some((topic, payload))
}

#[cfg(test)]
mod tests {
    use super::parse_frame as parse;

    #[test]
    fn splits_topic_and_spec() {
        assert_eq!(parse("[MON1][imu/accel/x][{=f32}]"), Some(("imu/accel/x", "{=f32}")));
        assert_eq!(parse("[MON1][flat][{}]"), Some(("flat", "{}")));
    }

    #[test]
    fn spec_may_contain_delimiters() {
        // A multi-argument spec can itself contain `][` and a trailing `]`.
        assert_eq!(parse("[MON1][a/b][{=u8}][{=u8}]"), Some(("a/b", "{=u8}][{=u8}")));
    }

    #[test]
    fn parses_rendered_messages_too() {
        assert_eq!(parse("[MON1][imu/accel/x][1.023]"), Some(("imu/accel/x", "1.023")));
        // A derived `Format` enum renders with braces and spaces.
        assert_eq!(
            parse("[MON1][power/state][Charging { mv: 3700 }]"),
            Some(("power/state", "Charging { mv: 3700 }")),
        );
        // A slice payload contains brackets on both ends.
        assert_eq!(parse("[MON1][adc/buf][[1, 2, 3]]"), Some(("adc/buf", "[1, 2, 3]")));
    }

    #[test]
    fn rejects_non_monitor_frames() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("[MON1]"), None);
        assert_eq!(parse("[MON1][no-spec]"), None);
        assert_eq!(parse("[MON9][a][{}]"), None);
        assert_eq!(parse("received {=u8} bytes"), None);
        // Trailing `]` is required, so a truncated string does not parse.
        assert_eq!(parse("[MON1][a][{=f32}"), None);
    }
}
