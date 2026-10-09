use crate::coordinator::PromptField;
use crate::screen::load::{Card, Snapshot};
use crate::screen::state::{
    App, Columns, Command, Focus, Layer, ListPos, Side, TABS, columns, detail_len, side_len,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub text: String,
    pub command: Command,
    pub selected: bool,
    pub modal: bool,
}

struct Col {
    x: u16,
    w: u16,
}

pub fn plan(app: &App, snap: &Snapshot, width: u16, height: u16) -> Vec<Placed> {
    let mut out = Vec::new();
    if height == 0 || width == 0 {
        return out;
    }
    push(
        &mut out,
        0,
        0,
        width,
        "HERDR PROJECTS",
        Command::Nothing,
        (false, false),
    );
    if columns(width) == Columns::TooSmall {
        push(
            &mut out,
            0,
            2,
            width,
            "Terminal is too small",
            Command::Nothing,
            (false, false),
        );
        push(
            &mut out,
            0,
            3,
            width,
            &format!("{width}x{height}"),
            Command::Nothing,
            (false, false),
        );
        return out;
    }
    let body = 1..height;
    match columns(width) {
        Columns::Three => {
            let side = Col {
                x: 0,
                w: 34.min(width),
            };
            let rest = width.saturating_sub(side.w.saturating_add(1));
            let work_w = (rest / 2).max(1);
            let work = Col {
                x: side.w.saturating_add(1),
                w: work_w.min(rest),
            };
            let overview = Col {
                x: work.x.saturating_add(work.w),
                w: width.saturating_sub(work.x.saturating_add(work.w)),
            };
            place_side(&mut out, app, snap, &side, body.clone());
            place_work(&mut out, app, snap, &work, body.clone());
            place_overview(&mut out, app, snap, &overview, body);
        }
        Columns::Two => {
            let work_w = width / 2;
            let work = Col {
                x: 0,
                w: work_w.max(1),
            };
            let overview = Col {
                x: work.w,
                w: width.saturating_sub(work.w),
            };
            place_work(&mut out, app, snap, &work, body.clone());
            place_overview(&mut out, app, snap, &overview, body);
        }
        Columns::One => {
            let col = Col { x: 0, w: width };
            if app.one == Focus::Overview {
                place_overview(&mut out, app, snap, &col, body);
            } else {
                place_work(&mut out, app, snap, &col, body);
            }
        }
        Columns::TooSmall => {}
    }
    place_layer(&mut out, app, snap, width, height);
    out
}

pub fn side_rows(app: &App, snap: &Snapshot) -> Vec<(String, Command)> {
    let mut rows = Vec::new();
    if let Some(filter) = &app.filter {
        rows.push((format!("filter {filter}"), Command::Nothing));
    }
    rows.extend([
        ("New Project".into(), Command::OpenNew),
        ("All Projects".into(), Command::SelectSide(Side::All)),
        (
            format!("Needs you {}", snap.needs.len()),
            Command::SelectSide(Side::Needs),
        ),
        (
            format!("Inbox {}", snap.inbox.len()),
            Command::SelectSide(Side::Inbox),
        ),
    ]);
    match app.side {
        Side::All => {
            for (index, card) in snap.projects.iter().enumerate() {
                if !crate::screen::state::project_visible(
                    &card.name,
                    &card.slug,
                    app.filter.as_deref(),
                ) {
                    continue;
                }
                rows.push((
                    format!(
                        "{} {} {} {}",
                        card.slug, card.lifecycle, card.availability, card.binding
                    ),
                    Command::SelectProject(index),
                ));
            }
            for failed in &snap.failed {
                rows.push((
                    format!("import failed {} {}", failed.path, failed.message),
                    Command::Nothing,
                ));
            }
            rows.push(("Project menu".into(), Command::OpenMenu));
        }
        Side::Needs => {
            if snap.needs.is_empty() {
                rows.push(("none".into(), Command::Nothing));
            }
            for row in &snap.needs {
                let index = snap
                    .projects
                    .iter()
                    .position(|card| card.id == row.project_id)
                    .unwrap_or(0);
                rows.push((
                    format!("{} {} {}", row.slug, row.thread_id, row.title),
                    Command::SelectProject(index),
                ));
            }
        }
        Side::Inbox => {
            if snap.inbox.is_empty() {
                rows.push(("none".into(), Command::Nothing));
            }
            for row in &snap.inbox {
                rows.push((
                    format!("{} {}", row.summary, row.body),
                    Command::InboxDone(row.id.clone()),
                ));
            }
        }
    }
    debug_assert_eq!(rows.len(), side_len(snap, app.side, app.filter.as_deref()));
    rows
}

fn place_side(
    out: &mut Vec<Placed>,
    app: &App,
    snap: &Snapshot,
    col: &Col,
    rows: std::ops::Range<u16>,
) {
    let items = side_rows(app, snap);
    let selected = clamp(app.side_list.selected, items.len());
    let window = rows.len().max(1);
    let offset = visible_offset(app.side_list.offset, selected, items.len(), window);
    let focus = app.focus == Focus::Projects && app.stack.is_empty();
    for (slot, (text, command)) in items.into_iter().enumerate().skip(offset).take(window) {
        let y = rows.start + (slot - offset) as u16;
        if y >= rows.end {
            break;
        }
        push(
            out,
            col.x,
            y,
            col.w,
            &text,
            command,
            (focus && slot == selected, false),
        );
    }
}

fn place_work(
    out: &mut Vec<Placed>,
    app: &App,
    snap: &Snapshot,
    col: &Col,
    rows: std::ops::Range<u16>,
) {
    let card = app.card(snap);
    let mut lines: Vec<(String, Command, bool)> = vec![(
        "Current work and recent reports".into(),
        Command::Nothing,
        false,
    )];
    match card {
        None => lines.push(("no project".into(), Command::Nothing, false)),
        Some(card) => {
            lines.push((format!("Goal: {}", card.goal), Command::Nothing, false));
            lines.push((card.coordinator.clone(), Command::Nothing, false));
            let repos = card
                .resources
                .iter()
                .filter(|row| row.kind == "repository")
                .count();
            let machines: Vec<&str> = card
                .resources
                .iter()
                .filter(|row| row.kind == "machine")
                .map(|row| row.label.as_str())
                .collect();
            let profiles = card
                .resources
                .iter()
                .filter(|row| row.label.starts_with("harness profile"))
                .count();
            lines.push((format!("repositories: {repos}"), Command::Nothing, false));
            lines.push((
                format!("machines: {}", machines.join(", ")),
                Command::Nothing,
                false,
            ));
            lines.push((
                format!("harness profiles: {profiles}"),
                Command::Nothing,
                false,
            ));
            for thread in card.threads.iter().take(4) {
                lines.push((thread_text(thread), Command::Nothing, false));
            }
            for report in card.reports.iter().take(3) {
                lines.push((report.clone(), Command::Nothing, false));
            }
            for line in card
                .operations
                .iter()
                .chain(card.recovery.iter())
                .chain(card.running.iter())
            {
                lines.push((line.clone(), Command::Nothing, false));
            }
            if let Some(id) = app.project_id(snap) {
                if let Some(prompt) = app.prompts.get(&id) {
                    for (text, label) in &prompt.outgoing {
                        lines.push((
                            format!("you sent: {text} ({label})"),
                            Command::Nothing,
                            false,
                        ));
                    }
                }
                if let Some(notice) = app.notices.get(&id) {
                    lines.push((notice.clone(), Command::Nothing, false));
                }
            }
        }
    }
    let actions = work_actions(card);
    let tail_count = actions.len() + 1;
    let nav = card.and_then(|card| app.nav.get(&card.id));
    let action_at = nav
        .map(|nav| clamp(nav.work.selected, actions.len()))
        .unwrap_or(0);
    for (index, (text, command)) in actions.into_iter().enumerate() {
        lines.push((text, command, index == action_at));
    }
    let prompt_focused = app.focus == Focus::Prompt && app.stack.is_empty();
    let (word, draft) = prompt_bits(app, snap);
    lines.push((
        format!("{word}: {draft}"),
        if prompt_focused {
            Command::SubmitPrompt
        } else {
            Command::FocusPrompt
        },
        prompt_focused,
    ));
    let focus_work = app.focus == Focus::Work && app.stack.is_empty();
    let window = rows.end.saturating_sub(rows.start) as usize;
    if window > 0 && lines.len() > window {
        let tail_count = tail_count.min(window);
        let status_room = window - tail_count;
        let tail = lines.split_off(lines.len() - tail_count);
        let status_end = rows.start + status_room as u16;
        paint_lines(
            out,
            col,
            rows.start..status_end,
            lines.into_iter().take(status_room).collect(),
            focus_work,
        );
        paint_lines(out, col, status_end..rows.end, tail, focus_work);
    } else {
        paint_lines(out, col, rows, lines, focus_work);
    }
}

fn work_actions(card: Option<&Card>) -> Vec<(String, Command)> {
    let mut actions = vec![
        (
            "Open conversation in Herdr".into(),
            Command::OpenConversation,
        ),
        ("Start coordinator".into(), Command::StartCoordinator),
    ];
    if card.is_some_and(|card| card.setup_incomplete) {
        actions.push(("Finish setup".into(), Command::FinishSetup));
    }
    actions
}

fn place_overview(
    out: &mut Vec<Placed>,
    app: &App,
    snap: &Snapshot,
    col: &Col,
    rows: std::ops::Range<u16>,
) {
    if matches!(app.stack.last(), Some(Layer::Detail)) {
        place_detail(out, app, snap, col, rows);
        return;
    }
    let card = app.card(snap);
    let tab = card
        .and_then(|card| app.nav.get(&card.id).map(|nav| nav.tab))
        .unwrap_or(0);
    let mut y = rows.start;
    let label = TABS
        .iter()
        .enumerate()
        .map(|(index, name)| {
            if index == tab {
                format!("[{name}]")
            } else {
                (*name).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if y < rows.end {
        push(
            out,
            col.x,
            y,
            col.w,
            &label,
            Command::Nothing,
            (false, false),
        );
        y += 1;
    }
    let items = overview_rows(card, tab);
    let pos = card
        .and_then(|card| app.nav.get(&card.id))
        .map(crate::screen::state::tab_list)
        .cloned()
        .unwrap_or_default();
    let selected = clamp(pos.selected, items.len());
    let window = (rows.end.saturating_sub(y)) as usize;
    let offset = visible_offset(pos.offset, selected, items.len(), window.max(1));
    let focus = app.focus == Focus::Overview && app.stack.is_empty();
    for (slot, (text, command)) in items.into_iter().enumerate().skip(offset).take(window) {
        if y >= rows.end {
            break;
        }
        push(
            out,
            col.x,
            y,
            col.w,
            &text,
            command,
            (focus && slot == selected, false),
        );
        y += 1;
    }
}

fn overview_rows(card: Option<&Card>, tab: usize) -> Vec<(String, Command)> {
    let Some(card) = card else {
        return vec![("no project".into(), Command::Nothing)];
    };
    match tab {
        0 => {
            let mut rows: Vec<(String, Command)> = if card.threads.is_empty() {
                vec![("none".into(), Command::Nothing)]
            } else {
                card.threads
                    .iter()
                    .map(|thread| (thread_text(thread), Command::Inspect))
                    .collect()
            };
            rows.push(("Start thread".into(), Command::StartThreadForm));
            rows
        }
        1 => item_rows(
            &card.tasks,
            "none",
            |task| format!("{} {}", task.title, task.notes),
            Command::Notes,
        ),
        2 => item_rows(
            &card.library,
            "none",
            |file| format!("{} {}", file.label, file.text.lines().next().unwrap_or("")),
            Command::ReadLibrary,
        ),
        3 => item_rows(
            &card.prs,
            "none",
            |pr| {
                format!(
                    "{} recorded reference {} not a live check",
                    pr.title, pr.reference
                )
            },
            Command::Inspect,
        ),
        4 => item_rows(
            &card.routines,
            "none",
            |routine| {
                format!(
                    "{} {} {}",
                    routine.name,
                    routine.schedule,
                    if routine.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                )
            },
            Command::ToggleRoutine,
        ),
        _ => item_rows(
            &card.resources,
            "none",
            |row| format!("{} {}", row.kind, row.label),
            Command::OpenResource,
        ),
    }
}

fn item_rows<T>(
    rows: &[T],
    empty: &str,
    text: impl Fn(&T) -> String,
    command: Command,
) -> Vec<(String, Command)> {
    if rows.is_empty() {
        vec![(empty.into(), Command::Nothing)]
    } else {
        rows.iter()
            .map(|row| (text(row), command.clone()))
            .collect()
    }
}

fn place_detail(
    out: &mut Vec<Placed>,
    app: &App,
    snap: &Snapshot,
    col: &Col,
    rows: std::ops::Range<u16>,
) {
    let items = detail_rows(app, snap);
    let selected = card_nav(app, snap)
        .map(|nav| clamp(nav.detail.selected, items.len()))
        .unwrap_or(0);
    let tab = card_nav(app, snap).map(|nav| nav.tab).unwrap_or(0);
    let title = TABS.get(tab).copied().unwrap_or("Overview");
    let mut y = rows.start;
    if y < rows.end {
        push(
            out,
            col.x,
            y,
            col.w,
            title,
            Command::Nothing,
            (false, false),
        );
        y += 1;
    }
    for (index, (text, command)) in items.into_iter().enumerate() {
        if y >= rows.end {
            break;
        }
        push(
            out,
            col.x,
            y,
            col.w,
            &text,
            command,
            (index == selected, false),
        );
        y += 1;
    }
    let _ = detail_len();
}

pub fn detail_rows(app: &App, snap: &Snapshot) -> Vec<(String, Command)> {
    let card = app.card(snap);
    let tab = card_nav(app, snap).map(|nav| nav.tab).unwrap_or(0);
    let blank = || (".".into(), Command::Nothing);
    match tab {
        0 => {
            let thread = card.and_then(|card| {
                let index = card_nav(app, snap)
                    .map(|nav| clamp(nav.threads.selected, card.threads.len()))
                    .unwrap_or(0);
                card.threads.get(index)
            });
            let head = thread
                .map(|thread| format!("{} {} pane {}", thread.id, thread.status, thread.pane_id))
                .unwrap_or_else(|| "no thread".into());
            let mut rows = vec![
                (head, Command::Nothing),
                ("Start PR".into(), Command::StartPr),
                ("Open PR reference".into(), Command::OpenPr),
                ("Restart".into(), Command::Restart),
            ];
            if let Some(thread) = thread {
                for path in &thread.files {
                    rows.push((path.clone(), Command::CopyPath(path.clone())));
                }
            }
            rows
        }
        1 => {
            let task = card.and_then(|card| {
                let index = card_nav(app, snap)
                    .map(|nav| clamp(nav.tasks.selected, card.tasks.len()))
                    .unwrap_or(0);
                card.tasks.get(index)
            });
            vec![
                (
                    task.map(|task| task.notes.clone())
                        .unwrap_or_else(|| "no task".into()),
                    Command::Nothing,
                ),
                ("Delegate".into(), Command::Delegate),
                ("Complete task".into(), Command::CompleteTask),
                ("Drop task".into(), Command::DropTask),
            ]
        }
        2 => {
            let file = card.and_then(|card| {
                let index = card_nav(app, snap)
                    .map(|nav| clamp(nav.library.selected, card.library.len()))
                    .unwrap_or(0);
                card.library.get(index)
            });
            vec![
                (
                    file.as_ref()
                        .map(|file| {
                            format!("{} {}", file.label, file.text.lines().next().unwrap_or(""))
                        })
                        .unwrap_or_else(|| "no file".into()),
                    Command::Nothing,
                ),
                ("Open file".into(), Command::ReadLibrary),
                (
                    file.as_ref()
                        .map(|file| format!("Copy path {}", file.path))
                        .unwrap_or_else(|| "Copy path".into()),
                    file.map(|file| Command::CopyPath(file.path.clone()))
                        .unwrap_or(Command::Nothing),
                ),
                blank(),
            ]
        }
        3 => {
            let pr = card.and_then(|card| {
                let index = card_nav(app, snap)
                    .map(|nav| clamp(nav.prs.selected, card.prs.len()))
                    .unwrap_or(0);
                card.prs.get(index)
            });
            vec![
                (
                    pr.map(|pr| {
                        format!(
                            "{} recorded reference {} not a live check",
                            pr.title, pr.reference
                        )
                    })
                    .unwrap_or_else(|| "no recorded reference".into()),
                    Command::Nothing,
                ),
                ("Open PR reference".into(), Command::OpenPr),
                ("Start PR".into(), Command::StartPr),
                blank(),
            ]
        }
        4 => {
            let prompt = card
                .and_then(|card| {
                    let index = card_nav(app, snap)
                        .map(|nav| clamp(nav.routines.selected, card.routines.len()))
                        .unwrap_or(0);
                    card.routines.get(index)
                })
                .map(|routine| routine.prompt.clone())
                .filter(|prompt| !prompt.is_empty())
                .unwrap_or_else(|| "no prompt".into());
            vec![
                (prompt, Command::Nothing),
                ("Toggle routine".into(), Command::ToggleRoutine),
                blank(),
                blank(),
            ]
        }
        _ => vec![
            ("resource".into(), Command::Nothing),
            ("Open resource".into(), Command::OpenResource),
            blank(),
            blank(),
        ],
    }
}

fn place_layer(out: &mut Vec<Placed>, app: &App, snap: &Snapshot, width: u16, height: u16) {
    let Some(layer) = app.stack.last() else {
        return;
    };
    if matches!(layer, Layer::Detail) {
        return;
    }
    let box_w = 60.min(width.saturating_sub(2)).max(1);
    let x = width.saturating_sub(box_w) / 2;
    let rows = match layer {
        Layer::Help => help_rows(),
        Layer::Picker => side_rows(app, snap),
        Layer::New(form) => new_rows(form),
        Layer::Thread(form) => thread_rows(form),
        Layer::Field(form) => vec![
            (form.title.clone(), Command::Nothing),
            (form.value.clone(), Command::SubmitField),
        ],
        Layer::Menu => crate::screen::state::MENU
            .iter()
            .enumerate()
            .map(|(index, label)| {
                (
                    (*label).to_string(),
                    crate::screen::state::menu_command(index),
                )
            })
            .collect(),
        Layer::Safety => safety_layer_rows(app.card(snap)),
        Layer::SafetyPick(pick) => pick
            .options
            .iter()
            .map(|option| {
                (
                    option.clone(),
                    Command::SetSafety {
                        key: pick.key.clone(),
                        value: option.clone(),
                    },
                )
            })
            .collect(),
        Layer::Confirm { text, .. } => vec![
            (text.clone(), Command::Nothing),
            ("Yes".into(), Command::ConfirmYes),
            ("No".into(), Command::Cancel),
        ],
        Layer::Detail => Vec::new(),
    };
    let selected = layer_selected(app, layer, rows.len());
    for (y, (index, (text, command))) in (1u16..).zip(rows.into_iter().enumerate()) {
        if y >= height {
            break;
        }
        push(out, x, y, box_w, &text, command, (index == selected, true));
    }
}

fn safety_layer_rows(card: Option<&crate::screen::load::Card>) -> Vec<(String, Command)> {
    let Some(card) = card else {
        return vec![("no project".into(), Command::Nothing)];
    };
    if card.safety.is_empty() {
        return vec![("no safety settings".into(), Command::Nothing)];
    }
    card.safety
        .iter()
        .map(|row| {
            let shown = if row.note.is_empty() {
                row.value.clone()
            } else {
                format!("{} ({})", row.value, row.note)
            };
            (
                format!("{} {} [{}]", row.key, shown, row.source),
                Command::SafetyKey(row.key.clone()),
            )
        })
        .collect()
}

fn layer_selected(app: &App, layer: &Layer, len: usize) -> usize {
    let index = match layer {
        Layer::Picker => app.picker_list.selected,
        Layer::Menu => app.menu_list.selected,
        Layer::Safety => app.safety_list.selected,
        Layer::SafetyPick(pick) => pick.selected,
        Layer::New(form) => form.field,
        Layer::Thread(form) => form.field,
        Layer::Field(_) => 1,
        Layer::Confirm { .. } => 1,
        _ => 0,
    };
    clamp(index, len)
}

fn help_rows() -> Vec<(String, Command)> {
    [
        "P project selection",
        "g then 1-6 overview tab",
        "Enter selected action",
        "o open worker",
        "Esc back",
        "Tab move focus",
        "? help",
        "m project menu",
        "i routine prompt",
        "/ filter projects",
        "S sweep",
        "y copy path",
    ]
    .into_iter()
    .map(|line| (line.into(), Command::Nothing))
    .collect()
}

fn new_rows(form: &crate::screen::state::NewForm) -> Vec<(String, Command)> {
    let mut rows = vec![
        (format!("Name {}", form.name), Command::Nothing),
        (format!("Goal {}", form.goal), Command::Nothing),
        (format!("Repositories {}", form.repos), Command::Nothing),
    ];
    if form.advanced {
        rows.push((
            format!("Thread profile {}", form.thread_profile),
            Command::Nothing,
        ));
        rows.push((
            format!("Coordinator profile {}", form.coordinator_profile),
            Command::Nothing,
        ));
    }
    rows.push(("Advanced".into(), Command::ToggleAdvanced));
    rows.push(("Create".into(), Command::SubmitNew));
    rows.push(("Cancel".into(), Command::Cancel));
    if !form.error.is_empty() {
        rows.push((form.error.clone(), Command::Nothing));
    }
    rows
}

fn thread_rows(form: &crate::screen::state::ThreadForm) -> Vec<(String, Command)> {
    let mut rows = vec![
        (
            format!(
                "Kind {} {}",
                form.kind_name(),
                crate::screen::load::thread_effect(form.kind_name())
            ),
            Command::Nothing,
        ),
        (format!("Title {}", form.title), Command::Nothing),
        (format!("Task {}", form.task), Command::Nothing),
        (format!("Repository {}", form.repo), Command::Nothing),
        (format!("Profile {}", form.profile), Command::Nothing),
        (format!("Machine {}", form.machine), Command::Nothing),
        (format!("Pane {}", form.pane), Command::Nothing),
    ];
    let submit = if form.allocated.is_some() {
        Command::Restart
    } else {
        Command::SubmitThread
    };
    rows.push(("Start".into(), submit));
    rows.push(("Cancel".into(), Command::Cancel));
    if !form.error.is_empty() {
        rows.push((form.error.clone(), Command::Nothing));
    }
    rows
}

fn thread_text(thread: &crate::screen::load::ThreadLine) -> String {
    let unavailable = if thread.unavailable {
        " unavailable"
    } else {
        ""
    };
    let reference = if thread.pr.is_empty() {
        String::new()
    } else {
        format!(" recorded reference {} not a live check", thread.pr)
    };
    format!(
        "{} {} {}{unavailable}{reference}",
        thread.id, thread.title, thread.status
    )
}

fn prompt_bits(app: &App, snap: &Snapshot) -> (&'static str, String) {
    let Some(id) = app.project_id(snap) else {
        return ("Draft", String::new());
    };
    match app.prompts.get(&id) {
        None => ("Draft", String::new()),
        Some(prompt) => (field_word(prompt.field), prompt.draft.clone()),
    }
}

fn field_word(field: PromptField) -> &'static str {
    match field {
        PromptField::Draft => "Draft",
        PromptField::Submitting => "Submitting",
        PromptField::Confirmed => "Confirmed",
        PromptField::Refused => "Refused",
        PromptField::Uncertain => "Uncertain",
    }
}

fn card_nav<'a>(app: &'a App, snap: &Snapshot) -> Option<&'a crate::screen::state::Nav> {
    app.nav.get(&app.project_id(snap)?)
}

fn paint_lines(
    out: &mut Vec<Placed>,
    col: &Col,
    rows: std::ops::Range<u16>,
    lines: Vec<(String, Command, bool)>,
    focus_actions: bool,
) {
    for (y, (text, command, action_selected)) in (rows.start..).zip(lines) {
        if y >= rows.end {
            break;
        }
        let selected = focus_actions && action_selected && command != Command::Nothing
            || action_selected && matches!(command, Command::SubmitPrompt);
        push(out, col.x, y, col.w, &text, command, (selected, false));
    }
}

fn push(
    out: &mut Vec<Placed>,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    command: Command,
    marks: (bool, bool),
) {
    let (selected, modal) = marks;
    if width == 0 {
        return;
    }
    let text = fit(text, width as usize);
    if text.is_empty() {
        return;
    }
    out.push(Placed {
        x,
        y,
        width,
        text,
        command,
        selected,
        modal,
    });
}

fn fit(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

pub fn clamp(index: usize, len: usize) -> usize {
    if len == 0 { 0 } else { index.min(len - 1) }
}

pub fn activation(app: &App, snap: &Snapshot, width: u16, height: u16) -> Command {
    let lines = plan(app, snap, width, height);
    let modal = lines.iter().any(|line| line.modal);
    lines
        .iter()
        .rev()
        .find(|line| line.selected && (!modal || line.modal))
        .map(|line| line.command.clone())
        .unwrap_or(Command::Nothing)
}

pub fn click_at(
    app: &App,
    snap: &Snapshot,
    column: u16,
    row: u16,
    width: u16,
    height: u16,
) -> Command {
    let lines = plan(app, snap, width, height);
    let modal = lines.iter().any(|line| line.modal);
    for line in lines.iter().rev() {
        if modal && !line.modal {
            continue;
        }
        let end = line.x.saturating_add(line.width);
        if row == line.y && column >= line.x && column < end {
            return line.command.clone();
        }
    }
    Command::Nothing
}

fn visible_offset(offset: usize, selected: usize, len: usize, window: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut pos = ListPos { selected, offset };
    pos.set(selected, len, window);
    pos.offset
}
