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

    let mut increasing_1x: i32 = 0;
    let mut increasing_2x: i32 = 0;
    let mut increasing_4x: i32 = 0;
    let mut increasing_8x: i32 = 0;

    let mut decreasing_1x: i32 = 0;
    let mut decreasing_2x: i32 = 0;
    let mut decreasing_4x: i32 = 0;
    let mut decreasing_8x: i32 = 0;

    let mut ticks: u32 = 0;

    loop {
        // increasing
        defmt_monitor::monitor!("Counters/Increasing/Increasing1x", "{=i32}", increasing_1x);
        defmt_monitor::monitor!("Counters/Increasing/Increasing2x", "{=i32}", increasing_2x);
        defmt_monitor::monitor!("Counters/Increasing/Increasing4x", "{=i32}", increasing_4x);
        defmt_monitor::monitor!("Counters/Increasing/Increasing8x", "{=i32}", increasing_8x);
        
        // decreasing
        defmt_monitor::monitor!("Counters/Decreasing/Decreasing1x", "{=i32}", decreasing_1x);
        defmt_monitor::monitor!("Counters/Decreasing/Decreasing2x", "{=i32}", decreasing_2x);
        defmt_monitor::monitor!("Counters/Decreasing/Decreasing4x", "{=i32}", decreasing_4x);
        defmt_monitor::monitor!("Counters/Decreasing/Decreasing8x", "{=i32}", decreasing_8x);

        // this prints an ordinary defmt log every loop for the purposes of testing the logging page
        if ticks % 10 == 0 {
            defmt::info!("heartbeat! ticks={}", ticks);
        }

        increasing_1x += 1;
        increasing_2x += 2;
        increasing_4x += 4;
        increasing_8x += 8;

        decreasing_1x -= 1;
        decreasing_2x -= 2;
        decreasing_4x -= 4;
        decreasing_8x -= 8;

        ticks = ticks.wrapping_add(1);
        Timer::after(PERIOD).await;
    }
}
