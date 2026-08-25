//! Keyboard and mouse handling.
//!
//! Mouse routing is hit-testing against the rectangles [`crate::ui`] recorded on the
//! last draw, which is why every interactive region has to publish its area.

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;

use crate::model::{App, Tab};
use crate::ui::{Focus, UiState};

/// Returns `true` when the application should exit.
pub fn handle(event: Event, app: &mut App, ui: &mut UiState) -> bool {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => return key_press(key, app, ui),
        Event::Mouse(mouse) => mouse_event(mouse, app, ui),
        _ => {}
    }
    false
}

fn key_press(key: KeyEvent, app: &mut App, ui: &mut UiState) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('c') if ctrl => return true,
        KeyCode::Tab | KeyCode::BackTab => {
            app.tab = match app.tab {
                Tab::Monitor => Tab::Logs,
                Tab::Logs => Tab::Monitor,
            };
        }
        // Hand the current view to the user's pager, outside the alternate screen,
        // where the terminal's own selection and copy work normally.
        KeyCode::Char('p') => {
            ui.pager = Some(match app.tab {
                Tab::Logs => app.logs_as_text(),
                Tab::Monitor => ui
                    .selected_path()
                    .map(|path| app.topic_as_text(&path))
                    .unwrap_or_default(),
            });
        }
        KeyCode::Char('f') => match app.tab {
            Tab::Monitor => {
                ui.history_follow = !ui.history_follow;
                if ui.history_follow {
                    // A highlight left behind would point at a row the view has moved
                    // away from.
                    ui.history_selected = None;
                }
            }
            Tab::Logs => {
                ui.log_follow = !ui.log_follow;
                if ui.log_follow {
                    ui.log_selected = None;
                }
            }
        },
        KeyCode::Char('c') if app.tab == Tab::Logs => {
            app.logs.clear();
            ui.log_offset = 0;
        }
        KeyCode::Up => match (app.tab, ui.focus) {
            (Tab::Monitor, Focus::History) => move_history(app, ui, -1),
            (Tab::Monitor, Focus::Tree) => {
                ui.tree.key_up();
            }
            (Tab::Logs, _) => move_log(app, ui, -1),
        },
        KeyCode::Down => match (app.tab, ui.focus) {
            (Tab::Monitor, Focus::History) => move_history(app, ui, 1),
            (Tab::Monitor, Focus::Tree) => {
                ui.tree.key_down();
            }
            (Tab::Logs, _) => move_log(app, ui, 1),
        },
        // Right expands a collapsed branch, and otherwise hands the keyboard to the
        // history pane. `key_right`'s return value cannot drive this: it reports whether
        // the identifier was newly added to the opened set, which is true even for a
        // childless leaf, so the node's actual children have to be consulted.
        KeyCode::Right if app.tab == Tab::Monitor && ui.focus == Focus::Tree => {
            if expandable(ui) {
                ui.tree.key_right();
            } else {
                focus_history(app, ui);
            }
        }
        KeyCode::Left if app.tab == Tab::Monitor => {
            if ui.focus == Focus::History {
                ui.focus = Focus::Tree;
            } else {
                ui.tree.key_left();
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') if app.tab == Tab::Monitor => {
            match ui.focus {
                Focus::Tree => {
                    ui.tree.toggle_selected();
                }
                Focus::History => focus_history(app, ui),
            }
        }
        KeyCode::PageUp => page(app, ui, -1),
        KeyCode::PageDown => page(app, ui, 1),
        KeyCode::Home => match (app.tab, ui.focus) {
            (Tab::Monitor, Focus::History) => move_history(app, ui, isize::MIN / 2),
            (Tab::Monitor, Focus::Tree) => {
                ui.tree.select_first();
            }
            (Tab::Logs, _) => move_log(app, ui, isize::MIN / 2),
        },
        KeyCode::End => match (app.tab, ui.focus) {
            (Tab::Monitor, Focus::History) => move_history(app, ui, isize::MAX / 2),
            (Tab::Monitor, Focus::Tree) => {
                ui.tree.select_last();
            }
            (Tab::Logs, _) => {
                ui.log_follow = true;
                ui.log_selected = None;
            }
        },
        _ => {}
    }
    false
}

/// Whether the selected tree node is a branch that is currently closed.
fn expandable(ui: &UiState) -> bool {
    let selected = ui.tree.selected().to_vec();
    if selected.is_empty() || ui.tree.opened().contains(&selected) {
        return false;
    }
    ui.tree
        .flatten(&ui.tree_items)
        .iter()
        .find(|flat| flat.identifier == selected)
        .is_some_and(|flat| !flat.item.children().is_empty())
}

fn page(app: &mut App, ui: &mut UiState, direction: isize) {
    match (app.tab, ui.focus) {
        (Tab::Monitor, Focus::History) => {
            let lines = ui.history_view.max(1) as isize;
            move_history(app, ui, direction * lines);
        }
        (Tab::Monitor, Focus::Tree) => {
            for _ in 0..ui.hit.tree.height.max(1) {
                if direction < 0 {
                    ui.tree.key_up();
                } else {
                    ui.tree.key_down();
                }
            }
        }
        (Tab::Logs, _) => {
            let lines = ui.log_view.max(1) as isize;
            move_log(app, ui, direction * lines);
        }
    }
}

fn mouse_event(mouse: MouseEvent, app: &mut App, ui: &mut UiState) {
    let position = Position::new(mouse.column, mouse.row);

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            for (rect, tab) in ui.hit.tabs.clone() {
                if rect.contains(position) {
                    app.tab = tab;
                    return;
                }
            }
            if app.tab == Tab::Logs {
                if ui.hit.logs.contains(position) {
                    click_logs(app, ui, position);
                }
                return;
            }

            // `click_at` selects what was rendered there, and toggles open/closed when
            // the node was already selected — which is how clicking an expand arrow
            // behaves in MQTTUI.
            if ui.hit.tree.contains(position) {
                ui.focus = Focus::Tree;
                ui.tree.click_at(position);
            } else if ui.hit.history.contains(position) {
                click_history(app, ui, position);
            }
        }
        MouseEventKind::ScrollUp => scroll_at(app, ui, position, -3),
        MouseEventKind::ScrollDown => scroll_at(app, ui, position, 3),
        _ => {}
    }
}

/// Selects the history row under the pointer.
fn click_history(app: &mut App, ui: &mut UiState, position: Position) {
    // The pane's first row is the column header, so it addresses no sample.
    if position.y <= ui.hit.history.y {
        return;
    }
    let row = (position.y - ui.hit.history.y - 1) as usize;
    let index = ui.history_offset + row;
    if index < history_len(app, ui) {
        ui.focus = Focus::History;
        ui.history_follow = false;
        ui.history_selected = Some(index);
    }
}

/// Selects the log line under the pointer.
fn click_logs(app: &App, ui: &mut UiState, position: Position) {
    let row = (position.y - ui.hit.logs.y) as usize;
    let index = ui.log_offset + row;
    if index < app.logs.len() {
        ui.log_follow = false;
        ui.log_selected = Some(index);
    }
}

/// Number of samples in the history of the currently selected topic.
fn history_len(app: &App, ui: &UiState) -> usize {
    ui.selected_path()
        .and_then(|path| app.topics.get(&path))
        .map_or(0, |topic| topic.history.len())
}

fn focus_history(app: &App, ui: &mut UiState) {
    let len = history_len(app, ui);
    if len == 0 {
        return;
    }
    ui.focus = Focus::History;
    ui.history_follow = false;
    move_cursor(
        &mut ui.history_selected,
        &mut ui.history_offset,
        ui.history_view,
        len,
        0,
    );
}

fn move_history(app: &App, ui: &mut UiState, delta: isize) {
    let len = history_len(app, ui);
    ui.history_follow = false;
    move_cursor(
        &mut ui.history_selected,
        &mut ui.history_offset,
        ui.history_view,
        len,
        delta,
    );
}

fn move_log(app: &App, ui: &mut UiState, delta: isize) {
    ui.log_follow = false;
    move_cursor(
        &mut ui.log_selected,
        &mut ui.log_offset,
        ui.log_view,
        app.logs.len(),
        delta,
    );
}

/// Moves a cursor within a list, scrolling the viewport just enough to keep it visible.
///
/// A delta of zero clamps an existing cursor and brings it into view without moving it.
fn move_cursor(
    selected: &mut Option<usize>,
    offset: &mut usize,
    view: usize,
    len: usize,
    delta: isize,
) {
    if len == 0 {
        *selected = None;
        return;
    }
    // With no cursor yet, arrowing starts from the newest line rather than the top.
    let current = selected.unwrap_or(len - 1) as isize;
    let next = current.saturating_add(delta).clamp(0, len as isize - 1) as usize;
    *selected = Some(next);

    let view = view.max(1);
    if next < *offset {
        *offset = next;
    } else if next >= *offset + view {
        *offset = next + 1 - view;
    }
    *offset = (*offset).min(len.saturating_sub(view));
}

fn scroll_at(app: &mut App, ui: &mut UiState, position: Position, delta: isize) {
    match app.tab {
        Tab::Monitor => {
            if ui.hit.tree.contains(position) {
                if delta < 0 {
                    ui.tree.scroll_up(delta.unsigned_abs());
                } else {
                    ui.tree.scroll_down(delta as usize);
                }
            } else if ui.hit.history.contains(position) {
                scroll_history(app, ui, delta);
            }
        }
        Tab::Logs => scroll_logs(app, ui, delta),
    }
}

/// Wheel scrolling moves the viewport without disturbing the selected row.
fn scroll_history(app: &App, ui: &mut UiState, delta: isize) {
    let len = history_len(app, ui);
    ui.history_follow = false;
    ui.history_offset = offset_by(ui.history_offset, delta).min(len.saturating_sub(ui.history_view));
    if ui.history_offset >= len.saturating_sub(ui.history_view) {
        ui.history_follow = true;
    }
}

fn scroll_logs(app: &App, ui: &mut UiState, delta: isize) {
    ui.log_follow = false;
    ui.log_offset = offset_by(ui.log_offset, delta).min(app.logs.len().saturating_sub(ui.log_view));
    // Scrolling back to the end resumes following, which is what you want after
    // scrolling to the bottom of a live stream.
    if ui.log_offset >= app.logs.len().saturating_sub(ui.log_view) {
        ui.log_follow = true;
    }
}

fn offset_by(offset: usize, delta: isize) -> usize {
    if delta < 0 {
        offset.saturating_sub(delta.unsigned_abs())
    } else {
        offset.saturating_add(delta as usize)
    }
}
