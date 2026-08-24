//! Renders the UI to an in-memory backend so the layout can be inspected and asserted
//! without a terminal. Run with `--nocapture` to eyeball the frames.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use time::UtcOffset;

use crate::model::{App, Tab};
use crate::source::{DecodedFrame, SourceEvent, route};
use crate::ui::{self, Focus, UiState};

/// Builds an app populated through the real routing path, so the snapshot exercises
/// `parse_frame`, numeric detection and device-clock parsing rather than fake state.
fn populated() -> App {
    let mut app = App::new(UtcOffset::UTC, 2000, 5000);
    for tick in 0..120u64 {
        let ms = tick * 20;
        let stamp = format!("00:00:{:02}.{:03}", (ms / 1000) % 60, ms % 1000);
        let t = tick as f64 * 0.02;

        let mut feed = |message: String| {
            match route(DecodedFrame {
                message,
                timestamp: Some(stamp.clone()),
                level: Some("INFO".into()),
                location: Some("src/sensor.rs:118".into()),
            }) {
                SourceEvent::Sample { path, sample } => app.push_sample(&path, sample),
                SourceEvent::Log(line) => app.push_log(*line),
                _ => {}
            }
        };

        feed(format!("[MON1][imu/accel/x][{:.3}]", (t * 1.7).sin() * 2.0));
        feed(format!("[MON1][imu/accel/y][{:.3}]", (t * 0.9).cos() * 1.5));
        feed(format!("[MON1][power/battery_mv][{}]", 3700 + tick % 17));
        feed("[MON1][power/state][Charging { mv: 3711 }]".to_string());
        feed(format!("[MON1][sys/ready][{}]", tick % 40 < 20));
        if tick % 20 == 0 {
            feed(format!("i2c transaction {} ok", tick / 20));
        }
    }
    app.status = "attached (RTT channel 0)".into();
    app.startup.ready();
    app
}

fn render_buffer(app: &mut App, ui: &mut UiState) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(110, 34)).expect("test backend");
    terminal
        .draw(|frame| ui::draw(frame, app, ui))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn render(app: &mut App, ui: &mut UiState) -> String {
    let buffer = render_buffer(app, ui);
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(code: ratatui::crossterm::event::KeyCode, app: &mut App, ui: &mut UiState) {
    use ratatui::crossterm::event::{Event, KeyEvent, KeyModifiers};
    crate::input::handle(
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
        app,
        ui,
    );
}

fn select(ui: &mut UiState, path: &[&str]) {
    for depth in 1..path.len() {
        ui.tree
            .open(path[..depth].iter().map(|s| s.to_string()).collect());
    }
    ui.tree
        .select(path.iter().map(|s| s.to_string()).collect());
}

#[test]
fn monitor_tab_graphs_a_numeric_topic() {
    let mut app = populated();
    let mut ui = UiState::default();
    // First draw populates the tree items that `select` then addresses.
    render(&mut app, &mut ui);
    select(&mut ui, &["imu", "accel", "x"]);
    let frame = render(&mut app, &mut ui);
    println!("\n=== Monitor: numeric topic ===\n{frame}");

    assert!(frame.contains("Monitor"), "tab bar is missing");
    assert!(frame.contains("imu/accel/x"), "selected topic title is missing");
    assert!(frame.contains("Topics (5, 600 messages)"), "tree header is wrong");
    assert!(frame.contains("History (120"), "history header is missing");
    assert!(frame.contains("every ~"), "history interval is missing");
    assert!(frame.contains("Graph"), "graph pane is missing");
    assert!(frame.contains("device clock"), "graph should use the device clock");
    assert!(frame.contains("min"), "stats are missing");
    assert!(
        frame.contains("⠤") || frame.contains("⠒") || frame.contains("⢀"),
        "expected braille plot marks, got:\n{frame}"
    );
}

/// On arrival the panes must already show something: the first topic is expanded to
/// and selected rather than leaving the user on a collapsed root branch.
#[test]
fn first_topic_is_selected_automatically() {
    let mut app = populated();
    let mut ui = UiState::default();

    assert!(ui.select_first_topic(&app), "there are topics to select");
    assert_eq!(ui.selected_path().as_deref(), Some("imu/accel/x"));

    let frame = render(&mut app, &mut ui);
    assert!(!frame.contains("no topic selected"), "title should name the topic");
    assert!(!frame.contains("select a topic"), "value pane should be populated");
    assert!(frame.contains("Graph"), "graph pane should be populated");
}

/// Regression: the tree used to be rebuilt only when a *new topic* appeared, so once
/// every topic had been seen the values shown beside each leaf froze at whatever they
/// held on that frame while the right-hand panes kept updating.
#[test]
fn tree_labels_track_the_latest_value() {
    let mut app = App::new(UtcOffset::UTC, 2000, 5000);
    app.startup.ready();
    let mut ui = UiState::default();

    let feed = |app: &mut App, value: i32| {
        if let SourceEvent::Sample { path, sample } = route(DecodedFrame {
            message: format!("[MON1][Counters/Decreasing][{value}]"),
            timestamp: Some("00:00:01.000".into()),
            level: Some("INFO".into()),
            location: None,
        }) {
            app.push_sample(&path, sample);
        }
    };

    feed(&mut app, -12);
    // Same expansion the real app performs on first data.
    assert!(ui.select_first_topic(&app));
    let frame = render(&mut app, &mut ui);
    assert!(frame.contains("Decreasing = -12"), "first value missing:\n{frame}");

    // The topic set is unchanged here, which is exactly the case that used to freeze.
    feed(&mut app, -336);
    let frame = render(&mut app, &mut ui);
    assert!(
        frame.contains("Decreasing = -336"),
        "tree label did not follow the latest value:\n{frame}"
    );
    assert!(!frame.contains("Decreasing = -12"), "stale value still shown");
}

#[test]
fn monitor_tab_refuses_to_graph_a_format_enum() {
    let mut app = populated();
    let mut ui = UiState::default();
    render(&mut app, &mut ui);
    select(&mut ui, &["power", "state"]);
    let frame = render(&mut app, &mut ui);
    println!("\n=== Monitor: non-graphable topic ===\n{frame}");

    assert!(frame.contains("not graphable"), "expected the not-graphable notice");
    assert!(frame.contains("not numeric"), "expected stats to report non-numeric");
}

#[test]
fn logs_tab_shows_only_non_monitor_frames() {
    let mut app = populated();
    let mut ui = UiState::default();
    app.tab = Tab::Logs;
    let frame = render(&mut app, &mut ui);
    println!("\n=== Logs ===\n{frame}");

    assert!(frame.contains("i2c transaction"), "log lines are missing");
    assert!(frame.contains("INFO"), "log level is missing");
    assert!(frame.contains("src/sensor.rs:118"), "log location is missing");
    assert!(
        !frame.contains("MON1"),
        "monitor frames must not leak into the log pane:\n{frame}"
    );
}

/// Guards the one piece of geometry that is duplicated rather than derived: the tab
/// hitboxes are computed by mirroring `Tabs`' internal padding, so if that ever changes
/// upstream the clickable regions would silently drift off their labels.
#[test]
fn tab_hitboxes_land_on_their_rendered_labels() {
    let mut app = populated();
    let mut ui = UiState::default();
    let frame = render(&mut app, &mut ui);
    let tab_row = frame.lines().next().expect("tab row");

    assert_eq!(ui.hit.tabs.len(), 2);
    for ((rect, tab), label) in ui.hit.tabs.iter().zip(["Monitor", "Logs"]) {
        let column = tab_row.find(label).expect("label is rendered") as u16;
        let end = column + label.chars().count() as u16;
        assert!(
            rect.x <= column && end <= rect.right(),
            "hitbox {rect:?} for {tab:?} does not cover `{label}` at columns {column}..{end} \
             in row `{tab_row}`",
        );
    }
}

/// Clicking a tab label must actually switch tabs, end to end through the hit test.
#[test]
fn clicking_a_tab_label_switches_tabs() {
    use ratatui::crossterm::event::{
        Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    let mut app = populated();
    let mut ui = UiState::default();
    let frame = render(&mut app, &mut ui);
    let column = frame.lines().next().unwrap().find("Logs").unwrap() as u16;

    let quit = crate::input::handle(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }),
        &mut app,
        &mut ui,
    );
    assert!(!quit);
    assert_eq!(app.tab, Tab::Logs, "clicking `Logs` should switch to the logs tab");
}

/// Reads a pane's bottom-right readout, located by the pane's own border row.
fn indicator(frame: &str, border_row: u16) -> String {
    let line = frame.lines().nth(border_row as usize).unwrap_or_default();
    let start = line.find("Scroll ").unwrap_or_else(|| {
        panic!("no indicator on row {border_row} of:\n{frame}");
    });
    // The row continues into the neighbouring pane's border, so stop at this pane's
    // bottom-right corner.
    let rest = &line[start..];
    let end = rest.find('\u{256f}').unwrap_or(rest.len());
    rest[..end].trim().to_string()
}

/// Pulls the number out of `Scroll <n>/<total>` or `Line <n>/<total>`.
fn field(indicator: &str, label: &str) -> Option<usize> {
    let start = indicator.find(label)? + label.len();
    let rest = &indicator[start..];
    rest[..rest.find('/')?].trim().parse().ok()
}

/// The scroll indicator replaces the scrollbars, so it has to appear on every scrollable
/// pane and report a real position.
#[test]
fn every_scrollable_pane_reports_its_scroll_position() {
    let mut app = populated();
    let mut ui = UiState::default();
    ui.select_first_topic(&app);
    let frame = render(&mut app, &mut ui);
    println!("\n=== Monitor with scroll indicators ===\n{frame}");

    // Tree counts only the rows currently visible: imu, accel, x, y, power, sys.
    assert_eq!(indicator(&frame, ui.hit.tree.bottom()), "Scroll 1/6  Line 3/6");
    // History is following, so the viewport sits at the tail. No cursor yet, so no Line.
    let history = indicator(&frame, ui.hit.history.bottom());
    assert!(history.starts_with("Scroll "), "got {history}");
    assert!(!history.contains("Line "), "no cursor yet, so no Line label: {history}");
    assert!(!frame.contains('\u{2551}'), "scrollbar track should be gone");

    app.tab = Tab::Logs;
    let frame = render(&mut app, &mut ui);
    assert_eq!(indicator(&frame, ui.hit.logs.bottom()), "Scroll 1/6");
}

/// The point of splitting the two readouts: the wheel moves the viewport on its own,
/// and must leave the cursor exactly where it was.
#[test]
fn wheel_moves_scroll_but_not_the_cursor() {
    use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};

    let mut app = populated();
    let mut ui = UiState::default();
    ui.select_first_topic(&app);
    render(&mut app, &mut ui);
    press(KeyCode::Right, &mut app, &mut ui);

    let frame = render(&mut app, &mut ui);
    let before = indicator(&frame, ui.hit.history.bottom());
    assert_eq!(field(&before, "Line "), Some(120), "cursor on the newest sample");
    let scrolled_from = field(&before, "Scroll ").expect("scroll position");

    crate::input::handle(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: ui.hit.history.x + 4,
            row: ui.hit.history.y + 2,
            modifiers: KeyModifiers::NONE,
        }),
        &mut app,
        &mut ui,
    );

    let frame = render(&mut app, &mut ui);
    let after = indicator(&frame, ui.hit.history.bottom());
    assert_eq!(
        field(&after, "Scroll "),
        Some(scrolled_from - 3),
        "one wheel notch should move the viewport three lines: {after}"
    );
    assert_eq!(
        field(&after, "Line "),
        Some(120),
        "the wheel must not disturb the cursor: {after}"
    );
}

/// The Logs pane gets a cursor of its own, highlighted the same way.
#[test]
fn logs_pane_has_its_own_cursor() {
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;

    let mut app = populated();
    let mut ui = UiState::default();
    app.tab = Tab::Logs;
    render(&mut app, &mut ui);
    assert_eq!(ui.log_selected, None);

    // With no cursor yet, movement starts from the newest line.
    press(KeyCode::Up, &mut app, &mut ui);
    assert_eq!(ui.log_selected, Some(4), "6 log lines, so one up from the newest");
    assert!(!ui.log_follow, "moving the cursor stops auto-scrolling");

    let buffer = render_buffer(&mut app, &mut ui);
    let row = ui.hit.logs.y + (4 - ui.log_offset) as u16;
    assert_eq!(
        buffer[(ui.hit.logs.x + 2, row)].style().bg,
        Some(Color::Green),
        "selected log line is not highlighted"
    );

    let frame = render(&mut app, &mut ui);
    assert_eq!(indicator(&frame, ui.hit.logs.bottom()), "Scroll 1/6  Line 5/6");

    // End returns to following and clears the cursor.
    press(KeyCode::End, &mut app, &mut ui);
    assert!(ui.log_follow);
    assert_eq!(ui.log_selected, None);
}

/// Right hands the keyboard to the history pane, where the arrows then move a selection
/// that is highlighted the same way the tree highlights its own.
#[test]
fn right_arrow_focuses_history_and_arrows_select_rows() {
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;

    let mut app = populated();
    let mut ui = UiState::default();
    ui.select_first_topic(&app);
    render(&mut app, &mut ui);
    assert_eq!(ui.focus, Focus::Tree);

    // The selected node is a leaf, so Right cannot expand and falls through to focus.
    press(KeyCode::Right, &mut app, &mut ui);
    assert_eq!(ui.focus, Focus::History);
    assert_eq!(ui.history_selected, Some(119), "should land on the newest sample");
    assert!(!ui.history_follow, "taking focus must stop auto-scrolling");

    press(KeyCode::Up, &mut app, &mut ui);
    press(KeyCode::Up, &mut app, &mut ui);
    assert_eq!(ui.history_selected, Some(117));

    // The selected row must actually be painted green.
    let buffer = render_buffer(&mut app, &mut ui);
    let row = ui.hit.history.y + 1 + (117 - ui.history_offset) as u16;
    let cell = &buffer[(ui.hit.history.x + 2, row)];
    assert_eq!(cell.style().bg, Some(Color::Green), "selected row is not highlighted");

    // Left returns the keyboard to the tree.
    press(KeyCode::Left, &mut app, &mut ui);
    assert_eq!(ui.focus, Focus::Tree);
}

/// Right must still expand a collapsed branch rather than jumping straight to the data.
#[test]
fn right_arrow_expands_a_collapsed_branch_first() {
    use ratatui::crossterm::event::KeyCode;

    let mut app = populated();
    let mut ui = UiState::default();
    render(&mut app, &mut ui);
    ui.tree.select(vec!["power".to_string()]);

    press(KeyCode::Right, &mut app, &mut ui);
    assert_eq!(ui.focus, Focus::Tree, "expanding must not steal focus");
    assert!(ui.tree.opened().contains(&vec!["power".to_string()]));
}

#[test]
fn clicking_a_history_row_selects_it() {
    use ratatui::crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = populated();
    let mut ui = UiState::default();
    ui.select_first_topic(&app);
    render(&mut app, &mut ui);

    // Third data row of the pane, under the column header.
    let row = ui.hit.history.y + 3;
    crate::input::handle(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: ui.hit.history.x + 4,
            row,
            modifiers: KeyModifiers::NONE,
        }),
        &mut app,
        &mut ui,
    );

    assert_eq!(ui.focus, Focus::History);
    assert_eq!(ui.history_selected, Some(ui.history_offset + 2));
    assert!(!ui.history_follow);
}

/// The loading screen accumulates stages: finished ones keep their frozen time and a
/// `(done)` marker, the active one keeps ticking.
#[test]
fn startup_screen_lists_every_stage_with_its_own_timer() {
    let mut app = App::new(UtcOffset::UTC, 2000, 5000);
    let mut ui = UiState::default();

    app.startup.begin("flashing".into());
    app.startup.begin("waiting for RTT".into());

    let frame = render(&mut app, &mut ui);
    println!("\n=== Startup ===\n{frame}");

    assert!(frame.contains("Connecting"), "title missing:\n{frame}");
    assert!(frame.contains("flashing..."), "finished stage missing");
    assert!(frame.contains("(done)"), "finished stage is not marked");
    assert!(frame.contains("waiting for RTT..."), "active stage missing");
    // Only the finished stage is marked; the active one is still running.
    assert_eq!(frame.matches("(done)").count(), 1);
    // The normal UI must not be underneath it.
    assert!(!frame.contains("Topics ("), "loading screen should replace the panes");
}

/// A finished stage's timer must stop, or the display would misreport how long each
/// phase actually took.
#[test]
fn finished_stage_timers_freeze() {
    let mut app = App::new(UtcOffset::UTC, 2000, 5000);
    app.startup.begin("flashing".into());
    app.startup.begin("waiting for RTT".into());

    let frozen = app.startup.stages[0].elapsed();
    let running = app.startup.stages[1].elapsed();
    std::thread::sleep(std::time::Duration::from_millis(25));

    assert_eq!(app.startup.stages[0].elapsed(), frozen, "finished timer moved");
    assert!(app.startup.stages[0].is_done());
    assert!(
        app.startup.stages[1].elapsed() > running,
        "active timer should still be running"
    );
    assert!(!app.startup.stages[1].is_done());
}

/// A startup failure stays on the loading screen, so the full error is readable next to
/// the stages that already succeeded.
#[test]
fn startup_failure_shows_the_error_in_full() {
    let mut app = App::new(UtcOffset::UTC, 2000, 5000);
    let mut ui = UiState::default();

    app.startup.begin("connecting to STM32F446RETx".into());
    app.startup.begin("flashing".into());
    app.startup.fail(
        "flashing target/thumbv7em-none-eabihf/release/test-firmware: \
         no probe was found — is the board plugged in?"
            .into(),
    );

    let frame = render(&mut app, &mut ui);
    println!("\n=== Startup failure ===\n{frame}");

    assert!(frame.contains("Startup failed"), "title should report failure");
    assert!(frame.contains("(failed)"), "failing stage is not marked");
    assert!(frame.contains("(done)"), "earlier stage should still show as done");
    assert!(
        frame.contains("no probe was found"),
        "the error text must be readable:\n{frame}"
    );
    assert!(!app.startup.done, "a failure must not fall through to the normal UI");
}

#[test]
fn ready_replaces_the_loading_screen_with_the_panes() {
    let mut app = App::new(UtcOffset::UTC, 2000, 5000);
    let mut ui = UiState::default();

    app.startup.begin("waiting for RTT".into());
    assert!(render(&mut app, &mut ui).contains("Connecting"));

    app.startup.ready();
    let frame = render(&mut app, &mut ui);
    assert!(frame.contains("Topics ("), "normal UI should take over:\n{frame}");
    assert!(!frame.contains("Connecting"));
}
