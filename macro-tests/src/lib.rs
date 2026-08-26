//! Every form `monitor!` accepts, compiled but never run.
//!
//! The macro's parsing is not trivial — an optional named argument, and topics that may
//! be either a literal or a list to concatenate — so this exists to catch a regression in
//! any of those shapes at build time. `cargo check` is the whole assertion.

#![no_std]

use defmt_monitor::monitor;

/// The motivating case: a wrapper that stamps out one topic per chip, with the varying
/// segment arriving as a `literal` metavariable.
macro_rules! chip_diagnostic {
    ($chip:literal, $line:expr) => {
        monitor!(
            ["Segments/ServiceDiagnostics/ChipState/Chip", $chip, "/line"],
            desc = "per-chip diagnostic line",
            "{=u8}",
            $line
        );
    };
}

pub fn accepted_forms(byte: u8, value: f32, ready: bool, state: &Uptime) {
    // Plain literal topic, with and without a description.
    monitor!("imu/accel/x", "{=f32}", value);
    monitor!("imu/accel/y", desc = "lateral g", "{=f32}", value);

    // Concatenated topics, directly and through a macro_rules wrapper.
    monitor!(["bank", 3, "/ch", 'a'], "{=u8}", byte);
    chip_diagnostic!(0, byte);
    chip_diagnostic!(1, byte);

    // A concatenated description.
    monitor!("net/port", desc = ["default ", 8080], "{=u8}", byte);

    // Format specs beyond a plain primitive: display hints, bitfields, `Format`,
    // multiple arguments, and no arguments at all.
    monitor!("adc/raw", "{=u16:x}", 1234u16);
    monitor!("flags/bits", "{=0..3}", byte);
    monitor!("sys/ready", "{=bool}", ready);
    monitor!("sys/uptime", "{}", state);
    monitor!("net/addr", "{=u8}.{=u8}", byte, byte);
    monitor!("sys/heartbeat", "tick");
}

/// A type with a derived `Format`, to cover the `{}` spec.
#[derive(defmt::Format)]
pub struct Uptime {
    pub millis: u32,
}
