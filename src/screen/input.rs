use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::screen::compose;
use crate::screen::load::Snapshot;
use crate::screen::state::{self, App, Command, Focus, Layer};

pub fn on_key(app: &mut App, snap: &Snapshot, key: KeyEvent, width: u16, height: u16) -> Command {
    let command = key_command(app, snap, key, width, height);
    state::apply(app, command, snap, width)
}

pub fn key_command(
    app: &mut App,
    snap: &Snapshot,
    key: KeyEvent,
    width: u16,
    height: u16,
) -> Command {
    if key.kind != KeyEventKind::Press {
        return Command::Nothing;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Command::Quit;
    }
    if let Some(command) = layer_key(app, snap, &key, width, height) {
        return command;
    }
    if app.filter.is_some() && app.stack.is_empty() {
        return filter_key(app, snap, &key, width, height);
    }
    if prompt_editing(app) {
        return prompt_key(&key);
    }
    if app.g {
        app.g = false;
        if let KeyCode::Char(ch @ '1'..='6') = key.code {
            return Command::SelectTab((ch as u8 - b'1') as usize);
        }
    } else if key.code == KeyCode::Char('g') {
        app.g = true;
        return Command::Nothing;
    }
    match key.code {
        KeyCode::Char('P') => Command::OpenProjectSelection,
        KeyCode::Enter => compose::activation(app, snap, width, height),
        KeyCode::Char('o') => open_target(app, snap),
        KeyCode::Esc => Command::Cancel,
        KeyCode::Tab => Command::FocusNext,
        KeyCode::BackTab => Command::FocusPrev,
        KeyCode::Char('?') => Command::Help,
        KeyCode::Up => Command::Move(-1),
        KeyCode::Down => Command::Move(1),
        KeyCode::Char('m') => Command::OpenMenu,
        KeyCode::Char('/') => Command::StartFilter,
        KeyCode::Char('S') => Command::SweepPreview,
        KeyCode::Char('y') => copy_command(app, snap),
        KeyCode::Char('a') if threads_tab(app, snap) => Command::Ack,
        KeyCode::Char('s') if threads_tab(app, snap) => Command::Stop,
        KeyCode::Char('r') if threads_tab(app, snap) => Command::Restart,
        KeyCode::Char('x') if threads_tab(app, snap) => Command::Resolve,
        KeyCode::Char('t') if threads_tab(app, snap) => Command::StartThreadForm,
        KeyCode::Char('d') if tasks_tab(app, snap) => Command::Delegate,
        KeyCode::Char('c') if tasks_tab(app, snap) => Command::CompleteTask,
        KeyCode::Char('x') if tasks_tab(app, snap) => Command::DropTask,
        KeyCode::Char('i') if routines_tab(app, snap) => Command::Inspect,
        KeyCode::Char(ch @ '1'..='9') if threads_tab(app, snap) => {
            Command::NextLine((ch as u8 - b'1') as usize)
        }
        _ => Command::Nothing,
    }
}

pub fn click_command(
    app: &App,
    snap: &Snapshot,
    column: u16,
    row: u16,
    width: u16,
    height: u16,
) -> Command {
    compose::click_at(app, snap, column, row, width, height)
}

fn layer_key(
    app: &mut App,
    snap: &Snapshot,
    key: &KeyEvent,
    width: u16,
    height: u16,
) -> Option<Command> {
    let layer = app.stack.last()?;
    if matches!(layer, Layer::Picker) && app.filter.is_some() {
        return Some(filter_key(app, snap, key, width, height));
    }
    if key.code == KeyCode::Char('/')
        && !matches!(
            layer,
            Layer::New(_) | Layer::Thread(_) | Layer::Field(_) | Layer::Confirm { .. }
        )
    {
        return Some(Command::StartFilter);
    }
    let kind = match layer {
        Layer::Detail => 1,
        Layer::Help => 2,
        Layer::Confirm { .. } => 3,
        Layer::New(_) => 4,
        Layer::Thread(_) => 5,
        Layer::Field(_) => 6,
        Layer::Menu | Layer::Picker | Layer::Safety | Layer::SafetyPick(_) => 7,
    };
    Some(match kind {
        1 => detail_key(app, snap, key),
        2 => {
            if key.code == KeyCode::Esc {
                Command::Cancel
            } else {
                Command::Nothing
            }
        }
        3 => match key.code {
            KeyCode::Esc | KeyCode::Char('n' | 'N') => Command::Cancel,
            KeyCode::Enter | KeyCode::Char('y' | 'Y') => Command::ConfirmYes,
            _ => Command::Nothing,
        },
        4 => form_key(app, key, true),
        5 => form_key(app, key, false),
        6 => match key.code {
            KeyCode::Esc => Command::Cancel,
            KeyCode::Enter => Command::SubmitField,
            KeyCode::Char(ch) => Command::Insert(ch),
            KeyCode::Backspace => Command::Backspace,
            _ => Command::Nothing,
        },
        _ => match key.code {
            KeyCode::Esc => Command::Cancel,
            KeyCode::Enter => compose::activation(app, snap, width, height),
            KeyCode::Up => Command::Move(-1),
            KeyCode::Down => Command::Move(1),
            _ => Command::Nothing,
        },
    })
}

fn detail_key(app: &App, snap: &Snapshot, key: &KeyEvent) -> Command {
    match key.code {
        KeyCode::Esc => Command::Cancel,
        KeyCode::Enter => {
            let rows = compose::detail_rows(app, snap);
            let index = app
                .project_id(snap)
                .and_then(|id| app.nav.get(&id))
                .map(|nav| compose::clamp(nav.detail.selected, rows.len()))
                .unwrap_or(0);
            rows.get(index)
                .map(|(_, command)| command.clone())
                .unwrap_or(Command::Nothing)
        }
        KeyCode::Up => Command::Move(-1),
        KeyCode::Down => Command::Move(1),
        KeyCode::Char('o') => Command::OpenWorker,
        KeyCode::Char('y') => copy_command(app, snap),
        _ => Command::Nothing,
    }
}

fn filter_key(app: &App, snap: &Snapshot, key: &KeyEvent, width: u16, height: u16) -> Command {
    match key.code {
        KeyCode::Esc => {
            if app.filter.as_ref().is_some_and(|filter| !filter.is_empty()) || app.stack.is_empty()
            {
                Command::ClearFilter
            } else {
                Command::Cancel
            }
        }
        KeyCode::Backspace => Command::FilterBackspace,
        KeyCode::Up => Command::Move(-1),
        KeyCode::Down => Command::Move(1),
        KeyCode::Enter => compose::activation(app, snap, width, height),
        KeyCode::Char(ch) => Command::FilterInsert(ch),
        _ => Command::Nothing,
    }
}

fn open_target(app: &App, snap: &Snapshot) -> Command {
    if app.focus != Focus::Work {
        return Command::OpenWorker;
    }
    let card = app.card(snap);
    let actions = state::coordinator_actions(card);
    let selected = card
        .and_then(|card| app.nav.get(&card.id))
        .map(|nav| compose::clamp(nav.work.selected, actions.len()))
        .unwrap_or(0);
    actions
        .get(selected)
        .map(|(_, command)| command.clone())
        .unwrap_or(Command::OpenWorker)
}

fn copy_command(app: &App, snap: &Snapshot) -> Command {
    let Some(card) = app.card(snap) else {
        return Command::Nothing;
    };
    let Some(nav) = app.nav.get(&card.id) else {
        return Command::Nothing;
    };
    if nav.overview {
        return Command::Nothing;
    }
    if nav.tab == 0 {
        let thread = compose::thread_by_selection(card, nav.threads.selected)
            .filter(|thread| !thread.files.is_empty());
        if let Some(thread) = thread {
            let selected = nav.detail.selected;
            let index = selected.saturating_sub(4).min(thread.files.len() - 1);
            return Command::CopyPath(thread.files[index].clone());
        }
    }
    if nav.tab == 2
        && let Some(file) = card.library.get(nav.library.selected)
    {
        return Command::CopyPath(file.path.clone());
    }
    Command::Nothing
}

fn form_key(app: &mut App, key: &KeyEvent, new_project: bool) -> Command {
    match key.code {
        KeyCode::Esc => Command::Cancel,
        KeyCode::Enter => form_submit(app, new_project),
        KeyCode::Tab => Command::Move(1),
        KeyCode::BackTab => Command::Move(-1),
        KeyCode::Left | KeyCode::Right => {
            if new_project {
                Command::ToggleAdvanced
            } else {
                cycle_kind(app, matches!(key.code, KeyCode::Right));
                Command::Nothing
            }
        }
        KeyCode::Char(ch) => Command::Insert(ch),
        KeyCode::Backspace => Command::Backspace,
        _ => Command::Nothing,
    }
}

fn form_submit(app: &App, new_project: bool) -> Command {
    if new_project {
        return Command::SubmitNew;
    }
    match app.stack.last() {
        Some(Layer::Thread(form)) if form.allocated.is_some() => Command::Restart,
        _ => Command::SubmitThread,
    }
}

fn cycle_kind(app: &mut App, forward: bool) {
    if let Some(Layer::Thread(form)) = app.stack.last_mut() {
        form.kind = if forward {
            (form.kind + 1) % 4
        } else {
            (form.kind + 3) % 4
        };
    }
}

fn prompt_key(key: &KeyEvent) -> Command {
    match key.code {
        KeyCode::Enter => Command::SubmitPrompt,
        KeyCode::Esc => Command::Cancel,
        KeyCode::Tab => Command::FocusNext,
        KeyCode::BackTab => Command::FocusPrev,
        KeyCode::Backspace => Command::Backspace,
        KeyCode::Char(ch) => Command::Insert(ch),
        _ => Command::Nothing,
    }
}

fn prompt_editing(app: &App) -> bool {
    app.stack.is_empty() && app.focus == Focus::Prompt
}

fn threads_tab(app: &App, snap: &Snapshot) -> bool {
    tab_is(app, snap, 0) && app.focus == Focus::Overview
}

fn tasks_tab(app: &App, snap: &Snapshot) -> bool {
    tab_is(app, snap, 1) && app.focus == Focus::Overview
}

fn routines_tab(app: &App, snap: &Snapshot) -> bool {
    tab_is(app, snap, 4) && app.focus == Focus::Overview
}

fn tab_is(app: &App, snap: &Snapshot, tab: usize) -> bool {
    app.project_id(snap).is_some_and(|id| {
        app.nav
            .get(&id)
            .is_some_and(|nav| !nav.overview && nav.tab == tab)
    })
}

#[cfg(test)]
pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
