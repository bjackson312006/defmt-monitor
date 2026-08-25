//! Nucleo-F446RE firmware that publishes two counters through `defmt-monitor`.
//!
//! Flash and watch it with:
//!
//! ```console
//! cargo build --release
//! defmt-monitor-tui --chip STM32F446RETx target/thumbv7em-none-eabihf/release/test-firmware
//! ```

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
// Pulled in for their side effects: the defmt transport and the panic handler.
use {defmt_rtt as _, panic_probe as _};

/// How often both counters are published.
const PERIOD: Duration = Duration::from_millis(100);

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_stm32::init(Default::default());
    defmt::info!("nucleo-f446re up, publishing counters every {} ms", PERIOD.as_millis());

    let mut increasing: i32 = 0;
    let mut decreasing: i32 = 0;
    let mut ticks: u32 = 0;

    loop {
        defmt_monitor::monitor!("Counters/Increasing", "{=i32}", increasing);
        defmt_monitor::monitor!("Counters/Decreasing", "{=i32}", decreasing);

        // An ordinary log every few seconds, so the TUI's Logs tab has traffic to show
        // and it is visible that monitor frames are being kept out of it.
        if ticks % 50 == 0 {
            defmt::info!("heartbeat: {} samples published", ticks);
        }

        increasing = increasing.wrapping_add(1);
        decreasing = decreasing.wrapping_sub(1);
        ticks = ticks.wrapping_add(1);
        Timer::after(PERIOD).await;
    }
}
