# defmt-monitor

A "monitor" view for embedded telemetry, in the spirit of [MQTTUI] — but for values
published over [defmt] instead of MQTT. Firmware publishes named values to `/`-separated
topics; a host-side TUI renders them as a live tree.

[MQTTUI]: https://github.com/EdJoPaTo/mqttui
[defmt]: https://defmt.ferrous-systems.com

## Layout

| Crate | Side | Status |
| --- | --- | --- |
| [`defmt-monitor`](defmt-monitor) | embedded | implemented |
| [`defmt-monitor-macros`](defmt-monitor-macros) | embedded (proc macro impl) | implemented |
| [`test-firmware`](test-firmware) | Nucleo-F446RE example firmware, not published | implemented |
| [`defmt-monitor-tui`](defmt-monitor-tui) | host | implemented |

## Firmware side

```rust
defmt_monitor::monitor!("imu/accel/x", "{=f32}", accel.x);
defmt_monitor::monitor!("power/battery_mv", "{=u16}", battery_mv);
```

Each call expands to one ordinary `defmt::info!`, so monitor frames ride the transport
the application already set up (typically `defmt-rtt`). There is no init function and no
extra RTT channel — see [Design notes](#design-notes).

## Host side

```console
# flash, reset, then monitor — replaces `cargo run` as your runner
defmt-monitor-tui --chip STM32F411CEUx target/thumbv7em-none-eabihf/debug/firmware

# attach to a target that is already flashed and running
defmt-monitor-tui --attach --chip STM32F411CEUx target/.../firmware

# no hardware needed: synthetic topics, logs and graphs
defmt-monitor-tui --demo
```

The ELF is required because defmt is a binary protocol — the decoder reconstitutes
messages from the `.defmt` table in the ELF, so it must match what is flashed.

**Startup.** Until the source reports itself ready, a centred panel lists each phase with
its own timer — running stages tick, finished ones freeze and are marked `(done)` — so a
stall shows up as the number that keeps climbing:

```text
╭ Connecting ──────────────────────────────────╮
│                                              │
│ flashing...                  5.912  (done)   │
│ waiting for RTT...           0.135           │
│                                              │
╰──────────────────────────────────────────────╯
```

A failure keeps the list on screen and prints the error in full beneath it, rather than
truncating it into the footer.

**Monitor tab.** A `/`-nested topic tree on the left; on the right the current value with
live min/max/mean, a scrollable timestamped history headed
`History (<count>, every ~<interval>)`, and a graph. Topics whose values are not numeric
— a `#[derive(Format)]` enum, say — say so instead of drawing a misleading plot.

**Logs tab.** Every non-monitor defmt frame in the usual console format, with level
colouring, source location and scrollback.

The keyboard moves between two panes. `→` expands a collapsed branch, and on a topic
hands the keyboard to the history pane, where the arrows move a highlighted selection;
`←` gives it back to the tree. The focused pane's selection is green, the unfocused one's
grey.

| | |
| --- | --- |
| `q` / `Esc` | quit |
| `Tab` | switch tab |
| `→` | expand a branch, or focus the history |
| `←` | collapse a branch, or return to the tree |
| `↑` `↓` | move the cursor in the focused pane |
| `Enter` / `Space` | toggle a node |
| `PgUp` / `PgDn`, `Home` / `End` | jump within the focused pane |
| `f` | toggle follow |
| `c` | clear logs |

Clicking works throughout: tab labels, tree rows and their expand arrows, and individual
history rows, which selects and highlights them. The wheel scrolls whichever pane is
under the pointer, without disturbing the selection. Scrolling back to the bottom of a
live pane re-enables follow.

Each scrollable pane reports two positions in its bottom-right corner rather than drawing
a scrollbar, because they move independently: `Scroll` is the line the viewport starts at,
which the wheel moves on its own, and `Line` is the cursor, which the arrows move. `Line`
is omitted until something is selected.

```text
╰─────────── Scroll 1/6  Line 3/6 ╯
```

A scrollbar thumb quantises to whole cells, so in a long history it cannot show
single-line movement at all; the counters always do.

## Example firmware

[`test-firmware/`](test-firmware) is a working embassy application for a Nucleo-F446RE
(STM32F446RE) that publishes `Counters/Increasing` and `Counters/Decreasing` every 100 ms,
plus a periodic heartbeat log. It is its own cargo workspace, because it cross-compiles
to `thumbv7em-none-eabihf` with linker flags that must not leak into the host crates — so
build it from inside that directory, where its `.cargo/config.toml` applies:

```console
cd test-firmware
cargo build --release
cargo run --release            # flash + plain defmt console, via probe-rs

# or watch it in the monitor TUI
defmt-monitor-tui --chip STM32F446RETx \
    target/thumbv7em-none-eabihf/release/test-firmware
```

It enables `embassy-time`'s `defmt-timestamp-uptime-us` feature, which defines the
once-per-binary `defmt::timestamp!`, so the TUI gets a real device clock for its graphs.

## Wire format

`monitor!` folds the topic into the defmt *format string* rather than passing it as an
argument. defmt interns format strings into the ELF's `.defmt` section at compile time
and transmits only an index, so the topic costs zero bytes at runtime regardless of
length. A `{=f32}` sample is 2 bytes of format index plus 4 bytes of payload.

The interned string is:

```text
[MON1][<topic>][<value format spec>]
```

The host decodes frames normally — it needs the ELF either way — and routes each one by
testing its format string with `defmt_monitor::parse_frame`, which returns
`Some((topic, spec))` for monitor frames and `None` for everything else. The host crate
should depend on `defmt-monitor` for that function and for `SENTINEL`, so the two sides
cannot drift.

Matching against the interned string rather than rendered output means the routing is
unaffected by the application's `defmt.toml`, `--log-format`, timestamp configuration, or
any other host-side display setting.

## Design notes

**No dedicated RTT channel.** `#[defmt::global_logger]` is a singleton per binary, so a
library cannot claim a second channel without owning the application's entire logging
transport. Monitor frames therefore share channel 0 with ordinary logs and are separated
host-side by format string, which is an exact match rather than a substring sniff.

The one real cost is a shared RTT ring buffer: at high sample rates monitor traffic will
crowd out log messages, since `defmt-rtt` drops on overflow. Raise
`DEFMT_RTT_BUFFER_SIZE` before reaching for a bigger change. If that stops being enough,
the migration path is a drop-in `defmt-rtt` replacement that multiplexes two channels;
`SENTINEL` is versioned so old firmware stays recognisable.

**`DEFMT_LOG` is required, not optional.** defmt's compile-time filter defaults to
`ERROR` when the variable is unset, so firmware that never sets it compiles away every
`info!` — including every `monitor!` call — and the TUI shows an empty topic tree with no
error at build or run time. The filter is keyed on the crate containing the call site, so
`defmt-monitor` cannot set it on your behalf. Set it in your firmware's
`.cargo/config.toml`:

```toml
[env]
DEFMT_LOG = "trace"
```

Or enable the `level-error` feature, which emits at `error!` and survives everything
short of `DEFMT_LOG=off`.

**A probe is exclusive.** Unlike an MQTT broker, a debug probe serves one session, so the
TUI cannot attach alongside `probe-rs run`. It therefore flashes and runs the firmware
itself by default, presenting the monitor and the log stream as two tabs. Use `--attach`
when something else has already flashed the target.

**Two clocks, both recorded.** Every sample carries the firmware's `defmt::timestamp!`
value *and* host arrival time. Graphs and the history interval prefer the device clock,
because RTT is polled in batches and host timestamps collapse bursts of samples onto
near-identical values. When the firmware defines no `timestamp!` the TUI falls back to
host time and says so in the graph title; log lines mark host-derived timestamps with
`*`.
