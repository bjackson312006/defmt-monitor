### Disclaimer
As of right now, most of this project is straight up vibecoded since it's an experimental tooling thing. Commits that are primarily vibecoded are tagged with `(vibecoded)`. Thank you claude!

Also, obviously, this isn't an official defmt project.

## defmt-monitor
This project provides a "monitor" TUI for defmt logs. It's basically just a clone of MQTTUI, but for defmt messages. It looks like this:

<img width="1920" height="1020" alt="Screenshot from 2026-08-24 21-28-01" src="https://github.com/user-attachments/assets/23331c6a-a8bc-464d-88b0-7d683cd8dd3e" />

The main "Monitor" page is shown in the screenshot above. There is also a "Logs" page for traditional logs:

<img width="1920" height="1020" alt="image" src="https://github.com/user-attachments/assets/238bcfed-5e7a-42f0-8ce1-68e0215e8c41" />

## Crates
This repo is a monorepo with four crates:
- defmt-monitor-macros: proc macro crate for internal use
- defmt-monitor-tui: the host-side TUI app
- defmt-monitor: the firmware-side crate that provides the `defmt_monitor::monitor!()` macro
- test-firmware: test firmware project demonstrating the project

defmt-monitor and defmt-monitor-tui are the main two meant to be used.

### defmt-monitor
As mentioned, this is the firmware-side crate that formats monitor logs correctly for the TUI. It provides the `defmt_monitor::monitor!()` macro, which can be used to
set up monitor logs like this:
```rust
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
```
The first parameter is the "topic" name. These ofc aren't actual MQTT topics, but the same format is being used here. Nevertheless, this determines how the TUI will nest the "topics", and what labels data will be displayed with.
The second parameter is for the data formatter, and the third parameter is for the data itself. It is basically just like the normal defmt macros.

The `defmt_monitor::monitor!()` macro is not affected by any defmt logging levels. Monitor logs are enabled/disabled via the `DEFMT_MONITOR` env var. So, if you want to disable all `defmt_monitor::monitor!()` logs in
a project, you would put this in the project's `config.toml`:
```toml
# .cargo/config.toml
[env]
DEFMT_MONITOR = "off"

// note: when `DEFMT_MONITOR` isn't set, monitor logs are on by default
```
This is helpful in case you want to disable monitor logs, but want to keep normal defmt logs.

### defmt-monitor-tui
This is the host-side TUI app. To install it, you can run `cargo install --git ssh://git@github.com/bjackson312006/defmt-monitor.git defmt-monitor-tui`. You could also just clone the repo and build the crate in release mode if you want (though using `cargo install` is probably cleaner). To see commands, run `defmt-monitor-tui --help`.

To see how `defmt-monitor-tui` can be used as a cargo runner, see the `test-firmware` example (`.cargo/config.toml` and `.scripts/run.sh`). This example setup sets `cargo run` to trigger a traditional `probe-rs run` without the `defmt_monitor` stuff compiled in, and sets the `cargo monitor` command to compile the project with `defmt_monitor`, flash it, and then open up the TUI. This sort of setup is nice because it makes all the TUI stuff opt-in.

### defmt-monitor-macros
This is just an internal proc macro crate for use by `defmt-monitor`.

### test-firmware
This is a test firmware project that provides an example for using this project, as well as an example for setting up cargo runners (as mentioned in the `defmt-monitor-tui` section).
