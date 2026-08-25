//! Rendering. Every interactive region records its rectangle into [`Hit`] as it draws,
//! which is what lets mouse events be routed without a second layout pass.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, BorderType, Chart, Dataset, GraphType, Padding, Paragraph, Tabs, Wrap,
};
use time::UtcOffset;
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::model::{App, LogLine, Tab, Topic};

const ACCENT: Color = Color::Green;
const DIM: Color = Color::DarkGray;

/// Rectangles of everything clickable, refreshed every draw.
#[derive(Default, Clone)]
pub struct Hit {
    pub tabs: Vec<(Rect, Tab)>,
    pub tree: Rect,
    pub history: Rect,
    pub logs: Rect,
}

/// Which pane the keyboard is driving.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Focus {
    #[default]
    Tree,
    History,
}

/// UI state, kept apart from the data model so the model stays testable on its own.
pub struct UiState {
    pub tree: TreeState<String>,
    pub tree_items: Vec<TreeItem<'static, String>>,
    pub focus: Focus,
    pub history_offset: usize,
    /// Index into the selected topic's history, when the user has picked a row.
    pub history_selected: Option<usize>,
    /// Number of history rows the last render could fit, needed for scroll clamping.
    pub history_view: usize,
    pub history_len: usize,
    pub history_follow: bool,
    pub log_offset: usize,
    /// Index into the log buffer, when the user has picked a line.
    pub log_selected: Option<usize>,
    pub log_view: usize,
    pub log_follow: bool,
    pub hit: Hit,
    /// When false, mouse capture is released so the terminal's own click-drag selection
    /// works. Applied by the event loop, which owns the terminal.
    pub mouse_capture: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tree: TreeState::default(),
            tree_items: Vec::new(),
            focus: Focus::default(),
            history_offset: 0,
            history_selected: None,
            history_view: 0,
            history_len: 0,
            history_follow: true,
            log_offset: 0,
            log_selected: None,
            log_view: 0,
            log_follow: true,
            hit: Hit::default(),
            mouse_capture: true,
        }
    }
}

impl UiState {
    /// Full `/`-joined path of the selected tree node, if anything is selected.
    pub fn selected_path(&self) -> Option<String> {
        let selected = self.tree.selected();
        (!selected.is_empty()).then(|| selected.join("/"))
    }

    /// Expands to and selects the first real topic, so the panes have content as soon
    /// as any data arrives.
    ///
    /// `TreeState::select_first` would land on the first *root branch*, which is
    /// collapsed and carries no value, leaving the right-hand panes empty.
    pub fn select_first_topic(&mut self, app: &App) -> bool {
        let Some(path) = app.topics.keys().next() else {
            return false;
        };
        let segments: Vec<String> = path.split('/').map(str::to_string).collect();
        for depth in 1..segments.len() {
            self.tree.open(segments[..depth].to_vec());
        }
        self.tree.select(segments);
        true
    }
}

pub fn draw(frame: &mut Frame, app: &mut App, ui: &mut UiState) {
    let area = frame.area();
    if !app.startup.done {
        draw_startup(frame, area, app);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tabs
            Constraint::Length(1), // selected topic
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_tabs(frame, chunks[0], app, ui);
    draw_title(frame, chunks[1], app, ui);
    match app.tab {
        Tab::Monitor => draw_monitor(frame, chunks[2], app, ui),
        Tab::Logs => draw_logs(frame, chunks[2], app, ui),
    }
    draw_footer(frame, chunks[3], app, ui);
}

/// The centred loading screen shown until the source reports itself ready.
///
/// Every stage stays listed with its own timer — running stages tick, finished ones
/// freeze — so a stall is visible as the number that keeps climbing, and a failure keeps
/// the context of everything that already succeeded.
fn draw_startup(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    // Size the label column to the longest stage so the timers line up, rather than
    // letting a long chip name push its own timer out of the column.
    let label_width = app
        .startup
        .stages
        .iter()
        .map(|stage| stage.label.chars().count() + 3)
        .max()
        .unwrap_or(0)
        .max(24);

    let mut lines: Vec<Line> = app
        .startup
        .stages
        .iter()
        .map(|stage| {
            let (style, marker) = if stage.failed {
                (Style::default().fg(Color::Red).bold(), "(failed)")
            } else if stage.is_done() {
                (Style::default().fg(DIM), "(done)")
            } else {
                (Style::default().fg(ACCENT).bold(), "")
            };
            Line::from(vec![
                Span::styled(
                    format!("{:<label_width$}", format!("{}...", stage.label)),
                    style,
                ),
                Span::styled(format!("{:>8.3}", stage.elapsed().as_secs_f64()), style),
                Span::styled(format!("  {marker}"), Style::default().fg(DIM)),
            ])
        })
        .collect();

    if let Some(error) = &app.startup.error {
        lines.push(Line::from(""));
        for line in error.lines() {
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Red),
            )));
        }
    }

    // Wide enough for the stage columns, and wider still if an error needs the room.
    let width = lines
        .iter()
        .map(|line| line.width() as u16 + 4)
        .max()
        .unwrap_or(48)
        .clamp(48, area.width.max(1));
    // Errors wrap, so allow for that when reserving height.
    let wrapped: u16 = lines
        .iter()
        .map(|line| (line.width() as u16).div_ceil((width - 4).max(1)).max(1))
        .sum();
    let block_area = centered(area, width, wrapped + 4);

    let title = if app.startup.error.is_some() {
        Line::from(" Startup failed ").fg(Color::Red).bold()
    } else {
        Line::from(" Connecting ").fg(ACCENT).bold()
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .padding(Padding::uniform(1));
    let inner = block.inner(block_area);
    frame.render_widget(block, block_area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);

    let hint = if app.startup.error.is_some() {
        " q Quit "
    } else {
        " q Quit   waiting for the target "
    };
    frame.render_widget(
        Paragraph::new(hint.fg(DIM)).alignment(Alignment::Center),
        rows[1],
    );
}

/// Centres a box of the given size, clamped to what is available.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App, ui: &mut UiState) {
    let titles: Vec<Line> = Tab::ALL.iter().map(|t| Line::from(t.title())).collect();
    let selected = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .style(Style::default().fg(DIM))
            .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
            .divider("|"),
        area,
    );

    // Mirror how `Tabs` lays titles out: one space of padding either side of each
    // title, then a single-cell divider between them.
    ui.hit.tabs.clear();
    let mut x = area.x;
    for tab in Tab::ALL {
        let width = tab.title().chars().count() as u16 + 2;
        ui.hit.tabs.push((Rect::new(x, area.y, width, 1), tab));
        x += width + 1;
    }
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App, ui: &UiState) {
    let text = match app.tab {
        Tab::Monitor => ui.selected_path().unwrap_or_else(|| "no topic selected".into()),
        Tab::Logs => format!("{} log messages", app.logs.len()),
    };
    frame.render_widget(
        Paragraph::new(Line::from(text).fg(Color::White).bold()).alignment(Alignment::Center),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, ui: &UiState) {
    let keys = if ui.mouse_capture {
        "q Quit  Tab Switch  → Data  ← Topics  ↑↓ Navigate  f Follow  c Clear  m Mouse"
    } else {
        "m Mouse on  —  mouse capture released, drag to select and copy as usual"
    };
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(
            app.status.chars().count() as u16 + 2,
        )])
        .split(area);
    let key_style = if ui.mouse_capture {
        Style::default().fg(DIM)
    } else {
        Style::default().fg(Color::Yellow)
    };
    frame.render_widget(Paragraph::new(Line::from(keys).style(key_style)), layout[0]);
    frame.render_widget(
        Paragraph::new(format!(" {} ", app.status).fg(Color::Black).bg(ACCENT)),
        layout[1],
    );
}

// ---------------------------------------------------------------- monitor tab

fn draw_monitor(frame: &mut Frame, area: Rect, app: &mut App, ui: &mut UiState) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(area);

    draw_tree(frame, columns[0], app, ui);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),      // value + stats
            Constraint::Percentage(45), // history
            Constraint::Min(6),         // graph
        ])
        .split(columns[1]);

    let topic = ui.selected_path().and_then(|p| app.topics.get(&p));
    draw_value(frame, rows[0], topic);
    draw_history(frame, rows[1], topic, ui, app.tz);
    draw_graph(frame, rows[2], topic);
}

fn draw_tree(frame: &mut Frame, area: Rect, app: &mut App, ui: &mut UiState) {
    if app.topics_dirty {
        ui.tree_items = build_tree_items(app);
        app.topics_dirty = false;
    }

    let flat = ui.tree.flatten(&ui.tree_items);
    let selected = flat
        .iter()
        .position(|item| item.identifier == ui.tree.selected());

    let title = format!(
        " Topics ({}, {} messages) ",
        app.topics.len(),
        app.total_messages()
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(title).fg(ACCENT).bold())
        .title_bottom(scroll_indicator(ui.tree.get_offset(), selected, flat.len()));
    ui.hit.tree = block.inner(area);

    let tree = Tree::new(&ui.tree_items)
        .expect("tree identifiers are unique by construction")
        .block(block)
        .highlight_style(selection_style(ui.focus == Focus::Tree))
        .experimental_scrollbar(None);
    frame.render_stateful_widget(tree, area, &mut ui.tree);
}

/// Builds nested tree items from the flat topic map.
///
/// A node can be both a topic and a parent (firmware may publish `a/b` and `a/b/c`),
/// which is why the trie is built first rather than assuming leaves only.
fn build_tree_items(app: &App) -> Vec<TreeItem<'static, String>> {
    #[derive(Default)]
    struct Node {
        children: BTreeMap<String, Node>,
        is_topic: bool,
    }

    let mut root = Node::default();
    for path in app.topics.keys() {
        let mut node = &mut root;
        for segment in path.split('/') {
            node = node.children.entry(segment.to_string()).or_default();
        }
        node.is_topic = true;
    }

    fn convert(
        app: &App,
        name: &str,
        node: &Node,
        prefix: &mut Vec<String>,
    ) -> TreeItem<'static, String> {
        prefix.push(name.to_string());

        let value = node
            .is_topic
            .then(|| app.topics.get(&prefix.join("/")))
            .flatten()
            .and_then(|t| t.latest())
            .map(|s| s.value.clone());

        let children: Vec<_> = node
            .children
            .iter()
            .map(|(child_name, child)| convert(app, child_name, child, prefix))
            .collect();

        let mut spans = vec![Span::styled(
            name.to_string(),
            Style::default().fg(Color::White),
        )];
        if let Some(value) = value {
            spans.push(Span::raw(" = "));
            spans.push(Span::styled(value, Style::default().fg(ACCENT)));
        } else {
            let topics = app.descendants(prefix).count();
            let messages: u64 = app.descendants(prefix).map(|t| t.total).sum();
            spans.push(Span::styled(
                format!("  {topics} topics, {messages} messages"),
                Style::default().fg(DIM),
            ));
        }

        let item = if children.is_empty() {
            TreeItem::new_leaf(name.to_string(), Line::from(spans))
        } else {
            TreeItem::new(name.to_string(), Line::from(spans), children)
                .expect("child identifiers are unique within a node")
        };
        prefix.pop();
        item
    }

    let mut prefix = Vec::new();
    root.children
        .iter()
        .map(|(name, node)| convert(app, name, node, &mut prefix))
        .collect()
}

fn draw_value(frame: &mut Frame, area: Rect, topic: Option<&Topic>) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(" Value ").fg(ACCENT).bold());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(topic) = topic else {
        frame.render_widget(
            Paragraph::new("select a topic".fg(DIM)).alignment(Alignment::Center),
            inner,
        );
        return;
    };
    let Some(latest) = topic.latest() else {
        frame.render_widget(Paragraph::new("no samples yet".fg(DIM)), inner);
        return;
    };

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    let left = vec![
        Line::from(latest.value.clone().fg(Color::White).bold()),
        Line::from(""),
        Line::from(format!("{} samples", topic.total).fg(DIM)),
    ];
    frame.render_widget(Paragraph::new(left), columns[0]);

    let right = if let Some(stats) = topic.stats() {
        vec![
            stat_line("min", format!("{:.4}", stats.min)),
            stat_line("max", format!("{:.4}", stats.max)),
            stat_line("mean", format!("{:.4}", stats.mean)),
            stat_line("numeric", format!("{} of {}", stats.count, topic.history.len())),
            stat_line(
                "clock",
                if latest.device_time.is_some() {
                    "device".to_string()
                } else {
                    "host".to_string()
                },
            ),
        ]
    } else {
        vec![
            Line::from("not numeric".fg(DIM)),
            stat_line("retained", topic.history.len().to_string()),
        ]
    };
    frame.render_widget(Paragraph::new(right), columns[1]);
}

fn stat_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), Style::default().fg(DIM)),
        Span::styled(value, Style::default().fg(Color::White)),
    ])
}

fn draw_history(
    frame: &mut Frame,
    area: Rect,
    topic: Option<&Topic>,
    ui: &mut UiState,
    tz: UtcOffset,
) {
    let title = match topic.and_then(|t| t.mean_interval().map(|i| (t, i))) {
        Some((topic, interval)) => {
            format!(" History ({}, every ~{}) ", topic.total, humanize(interval))
        }
        None => format!(" History ({}) ", topic.map_or(0, |t| t.total)),
    };

    let len = topic.map_or(0, |t| t.history.len());
    ui.history_len = len;

    // One row of the pane is spent on the column header.
    let view = area.height.saturating_sub(3) as usize;
    ui.history_view = view;
    if ui.history_follow {
        ui.history_offset = len.saturating_sub(view);
    }
    let offset = ui.history_offset.min(len.saturating_sub(view));
    ui.history_offset = offset;

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(title).fg(ACCENT).bold())
        .title_bottom(scroll_indicator(offset, ui.history_selected, len));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    ui.hit.history = inner;

    let Some(topic) = topic else {
        return;
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{:<14}", "Time"), Style::default().fg(DIM)),
        Span::styled(format!("{:<14}", "Device"), Style::default().fg(DIM)),
        Span::styled("Value", Style::default().fg(DIM)),
    ])];

    for (row, sample) in topic.history.iter().skip(offset).take(view).enumerate() {
        let index = offset + row;
        let selected = ui.history_selected == Some(index);
        let local = sample.host_time.to_offset(tz);

        let text = format!(
            "{:<14}{:<14}{}",
            format!(
                "{:02}:{:02}:{:02}.{:03}",
                local.hour(),
                local.minute(),
                local.second(),
                local.millisecond()
            ),
            sample.device_time_raw.clone().unwrap_or_else(|| "-".into()),
            sample.value,
        );

        if selected {
            // Padded to the full width so the highlight reads as a solid row, the same
            // way the tree highlights its selection.
            let pad = (inner.width as usize).saturating_sub(text.chars().count());
            lines.push(Line::from(Span::styled(
                format!("{text}{}", " ".repeat(pad)),
                selection_style(ui.focus == Focus::History),
            )));
        } else {
            let (time, rest) = text.split_at(14);
            let (device, value) = rest.split_at(14);
            lines.push(Line::from(vec![
                Span::styled(time.to_string(), Style::default().fg(Color::White)),
                Span::styled(device.to_string(), Style::default().fg(DIM)),
                Span::styled(value.to_string(), Style::default().fg(ACCENT)),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_graph(frame: &mut Frame, area: Rect, topic: Option<&Topic>) {
    let Some(topic) = topic else {
        frame.render_widget(placeholder("Graph", "select a topic"), area);
        return;
    };

    if !topic.is_graphable() {
        let reason = topic.latest().map_or_else(
            || "no samples yet".to_string(),
            |s| format!("this topic is not graphable — values render as `{}`", s.value),
        );
        frame.render_widget(placeholder("Graph", &reason), area);
        return;
    }

    let (points, device_clock) = topic.graph_points();
    if points.len() < 2 {
        frame.render_widget(placeholder("Graph", "waiting for more samples"), area);
        return;
    }

    let (mut x_min, mut x_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut y_min, mut y_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for (x, y) in &points {
        x_min = x_min.min(*x);
        x_max = x_max.max(*x);
        y_min = y_min.min(*y);
        y_max = y_max.max(*y);
    }
    // A flat signal would otherwise collapse to a zero-height axis.
    if (y_max - y_min).abs() < f64::EPSILON {
        y_min -= 0.5;
        y_max += 0.5;
    }
    if (x_max - x_min).abs() < f64::EPSILON {
        x_max = x_min + 1.0;
    }

    let clock = if device_clock { "device clock" } else { "host arrival (no device timestamp)" };
    let dataset = Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(ACCENT))
        .data(&points);

    let chart = Chart::new(vec![dataset])
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(Line::from(format!(" Graph — {clock} ")).fg(ACCENT).bold()),
        )
        .x_axis(
            Axis::default()
                .style(Style::default().fg(DIM))
                .bounds([x_min, x_max])
                .labels(vec![
                    Span::raw(format!("{x_min:.2}")),
                    Span::raw(format!("{x_max:.2}")),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(DIM))
                .bounds([y_min, y_max])
                .labels(vec![
                    Span::raw(format!("{y_min:.3}")),
                    Span::raw(format!("{y_max:.3}")),
                ]),
        );
    frame.render_widget(chart, area);
}

// ------------------------------------------------------------------- logs tab

fn draw_logs(frame: &mut Frame, area: Rect, app: &App, ui: &mut UiState) {
    // Borders take a column either side and a row top and bottom.
    let width = area.width.saturating_sub(2);
    let view = area.height.saturating_sub(2) as usize;

    // Following means the *tail* must be visible, and a wrapped entry can occupy several
    // rows, so the first visible entry is found by walking back from the end until the
    // viewport is full rather than by subtracting a fixed count.
    if ui.log_follow {
        let mut rows = 0usize;
        let mut first = app.logs.len();
        for (index, log) in app.logs.iter().enumerate().rev() {
            let height = wrapped_height(&log_line(log, app.tz, false), width);
            if rows + height > view && rows > 0 {
                break;
            }
            rows += height;
            first = index;
            if rows >= view {
                break;
            }
        }
        ui.log_offset = first;
    }
    let offset = ui.log_offset.min(app.logs.len().saturating_sub(1));
    ui.log_offset = offset;

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(
            Line::from(format!(
                " Logs ({}{}) ",
                app.logs.len(),
                if ui.log_follow { ", following" } else { "" }
            ))
            .fg(ACCENT)
            .bold(),
        )
        .title_bottom(scroll_indicator(offset, ui.log_selected, app.logs.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    ui.hit.logs = inner;

    let mut lines = Vec::new();
    let mut rows = 0usize;
    let mut shown = 0usize;
    for (index, log) in app.logs.iter().enumerate().skip(offset) {
        let line = log_line(log, app.tz, ui.log_selected == Some(index));
        let height = wrapped_height(&line, width);
        // Always show at least one entry, even if it alone overflows the pane.
        if rows + height > view && rows > 0 {
            break;
        }
        rows += height;
        shown += 1;
        lines.push(line);
    }
    // Scrolling and paging work in entries, not rows, so this is what input clamps to.
    ui.log_view = shown.max(1);

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

/// Renders one log entry. Selected entries are styled rather than padded to the pane
/// width, because padding would itself wrap onto a row of its own.
fn log_line(log: &LogLine, tz: UtcOffset, selected: bool) -> Line<'static> {
    let (colour, level) = match log.level.as_deref() {
        Some("ERROR") => (Color::Red, "ERROR"),
        Some("WARN") => (Color::Yellow, "WARN "),
        Some("INFO") => (Color::Green, "INFO "),
        Some("DEBUG") => (Color::Blue, "DEBUG"),
        Some("TRACE") => (DIM, "TRACE"),
        _ => (DIM, "     "),
    };
    // Firmware without `defmt::timestamp!` has no device clock, so fall back to host
    // arrival time, marked with `*` so the two are not confused.
    let stamp = log.timestamp.clone().unwrap_or_else(|| {
        let local = log.host_time.to_offset(tz);
        format!(
            "{:02}:{:02}:{:02}.{:03}*",
            local.hour(),
            local.minute(),
            local.second(),
            local.millisecond()
        )
    });
    let location = log
        .location
        .as_ref()
        .map(|l| format!("  {l}"))
        .unwrap_or_default();

    if selected {
        let style = selection_style(true);
        return Line::from(vec![
            Span::styled(format!("{stamp:<15}"), style),
            Span::styled(format!("{level} "), style),
            Span::styled(log.message.clone(), style),
            Span::styled(location, style),
        ]);
    }

    Line::from(vec![
        Span::styled(format!("{stamp:<15}"), Style::default().fg(DIM)),
        Span::styled(format!("{level} "), Style::default().fg(colour).bold()),
        Span::styled(log.message.clone(), Style::default().fg(Color::White)),
        Span::styled(location, Style::default().fg(DIM)),
    ])
}

/// Rows a log entry occupies once wrapped to `width`.
fn wrapped_height(line: &Line<'static>, width: u16) -> usize {
    Paragraph::new(line.clone())
        .wrap(Wrap { trim: false })
        .line_count(width)
        .max(1)
}

// -------------------------------------------------------------------- helpers

fn placeholder(title: &str, message: &str) -> Paragraph<'static> {
    Paragraph::new(message.to_string().fg(DIM))
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(Line::from(format!(" {title} ")).fg(ACCENT).bold()),
        )
}

/// The bottom-right readout of a scrollable pane.
///
/// Reports two independent positions, because they move independently: `Scroll` is the
/// line the viewport starts at, which the wheel moves on its own, and `Line` is the
/// cursor, which the arrows move. `Line` is omitted until something is selected.
///
/// This replaces a `Scrollbar`, whose thumb quantises to whole cells and so cannot show
/// movement of a single line through a long list.
fn scroll_indicator(offset: usize, selected: Option<usize>, total: usize) -> Line<'static> {
    if total == 0 {
        return Line::from("");
    }
    let scroll = (offset + 1).min(total);
    let text = match selected {
        Some(index) => format!(" Scroll {scroll}/{total}  Line {}/{total} ", index + 1),
        None => format!(" Scroll {scroll}/{total} "),
    };
    Line::from(Span::styled(text, Style::default().fg(DIM))).right_aligned()
}

/// Highlight for a selected row, dimmed when its pane does not have the keyboard.
fn selection_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White).bg(Color::DarkGray)
    }
}

fn humanize(seconds: f64) -> String {
    if seconds <= 0.0 {
        "?".to_string()
    } else if seconds < 1.0 {
        format!("{:.1} ms", seconds * 1000.0)
    } else if seconds < 90.0 {
        format!("{seconds:.1} seconds")
    } else {
        format!("{:.1} minutes", seconds / 60.0)
    }
}
