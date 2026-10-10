use crate::coordinator::PromptField;
use crate::screen::load::{Card, Snapshot};
use crate::screen::state::{
    App, Columns, Command, CoordinatorState, Focus, Layer, ListPos, Side, TABS, columns,
    coordinator_state, detail_len, showing_overview, side_len,
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

pub fn framed(width: u16, height: u16) -> bool {
    width >= 8 && height >= 6
}

pub fn frame_title(app: &App, snap: &Snapshot) -> String {
    format!(" {} ", screen_title(app, snap))
}

pub fn frame_status(app: &App, snap: &Snapshot, width: u16) -> String {
    let title = frame_title(app, snap);
    let status = format!(
        "{} Projects  {} Need you  {} Inbox ",
        snap.projects.len(),
        snap.needs.len(),
        snap.inbox.len()
    );
    if title.chars().count() + status.chars().count() + 4 >= width as usize {
        format!("{} need  {} inbox ", snap.needs.len(), snap.inbox.len())
    } else {
        status
    }
}

pub fn frame_footer(app: &App, width: u16) -> String {
    let text =
        if app.stack.last().is_some_and(|layer| {
            matches!(layer, Layer::New(_) | Layer::Thread(_) | Layer::Field(_))
        }) {
            "Enter confirm  Esc close"
        } else if app.focus == Focus::Prompt {
            "Enter send  Esc close"
        } else {
            match app.focus {
                Focus::Global => "Enter open  Tab focus  P projects",
                Focus::Projects => "Enter select  P projects  / filter  Tab focus",
                Focus::Work => "Enter run  o coordinator  Tab focus",
                Focus::Tabs => "Enter tab  Up Down feature  g1-6  Tab focus",
                Focus::Overview => "Enter inspect  o worker  g1-6  Tab focus",
                Focus::Prompt => "Enter send  Esc close",
            }
        };
    text.chars()
        .take(width.saturating_sub(4) as usize)
        .collect()
}

pub fn plan(app: &App, snap: &Snapshot, width: u16, height: u16) -> Vec<Placed> {
    let mut out = Vec::new();
    if height == 0 || width == 0 {
        return out;
    }
    if !crate::screen::state::cockpit_fits(width, height) {
        push(
            &mut out,
            0,
            0,
            width,
            "Terminal is too small",
            Command::Nothing,
            (false, false),
        );
        return out;
    }
    let frame = framed(width, height);
    let inset = u16::from(frame);
    let x0 = inset;
    let x1 = width.saturating_sub(inset);
    let inner_w = x1.saturating_sub(x0).max(1);
    let body = inset..height.saturating_sub(inset);
    let mut splits = Vec::new();
    match columns(width) {
        Columns::Three => {
            let side_w = 30.min(inner_w.saturating_sub(4));
            let sep1 = x0.saturating_add(side_w);
            let rest = inner_w.saturating_sub(side_w).saturating_sub(2);
            let work_w = (rest / 2).max(1);
            let sep2 = sep1.saturating_add(1).saturating_add(work_w);
            let side = Col { x: x0, w: side_w };
            let work = Col {
                x: sep1.saturating_add(1),
                w: work_w,
            };
            let overview = Col {
                x: sep2.saturating_add(1),
                w: x1.saturating_sub(sep2.saturating_add(1)),
            };
            splits.extend([sep1, sep2]);
            let below = head_row(&mut out, &side, body.clone(), "PROJECTS");
            place_side(&mut out, app, snap, &side, below);
            let below = head_row(&mut out, &work, body.clone(), &column_name(app, snap));
            place_work(&mut out, app, snap, &work, below);
            let below = head_row(&mut out, &overview, body.clone(), "OVERVIEW");
            place_overview(&mut out, app, snap, &overview, below);
        }
        Columns::Stacked => {
            let col = Col { x: x0, w: inner_w };
            let list_need = 1 + side_rows(app, snap).len() as u16;
            let work_need = 1 + work_lines(app, snap).0.len() as u16;
            let (list, work, feature) = stacked_bands(body.start, body.end, list_need, work_need);
            let below = head_row(&mut out, &col, list, "PROJECTS");
            place_side(&mut out, app, snap, &col, below);
            let below = head_row(&mut out, &col, work, &column_name(app, snap));
            place_work(&mut out, app, snap, &col, below);
            let below = head_row(&mut out, &col, feature, "OVERVIEW");
            place_overview(&mut out, app, snap, &col, below);
        }
        Columns::TooSmall => {
            push(
                &mut out,
                x0,
                body.start,
                inner_w,
                "Terminal is too small",
                Command::Nothing,
                (false, false),
            );
        }
    }
    if frame {
        for y in body {
            for x in &splits {
                push(&mut out, *x, y, 1, "│", Command::Nothing, (false, false));
            }
        }
    }
    place_layer(&mut out, app, snap, width, height);
    out
}

fn stacked_bands(
    start: u16,
    end: u16,
    list_need: u16,
    work_need: u16,
) -> (
    std::ops::Range<u16>,
    std::ops::Range<u16>,
    std::ops::Range<u16>,
) {
    let total = end.saturating_sub(start);
    let list_room = ((total as u32 * 40 / 100) as u16)
        .clamp(4, 8)
        .min(total.saturating_sub(8));
    let list = if list_room == 0 {
        0
    } else {
        list_need.min(list_room)
    };
    let rest = total.saturating_sub(list);
    let work_room = (rest / 2).clamp(4, 7).min(rest.saturating_sub(3));
    let work = if work_room == 0 {
        0
    } else {
        work_need.min(work_room)
    };
    let list_end = start.saturating_add(list);
    let work_end = list_end.saturating_add(work).min(end);
    (start..list_end, list_end..work_end, work_end..end)
}

fn head_row(
    out: &mut Vec<Placed>,
    col: &Col,
    rows: std::ops::Range<u16>,
    title: &str,
) -> std::ops::Range<u16> {
    if rows.start >= rows.end || col.w == 0 {
        return rows;
    }
    push(
        out,
        col.x,
        rows.start,
        col.w,
        title,
        Command::Nothing,
        (false, false),
    );
    rows.start.saturating_add(1)..rows.end
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
                rows.push((project_row(snap, card), Command::SelectProject(index)));
            }
            for failed in &snap.failed {
                let message = format!("{} {}", failed.message, failed.path);
                rows.push((message.clone(), Command::ShowImport { message }));
            }
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
                    Command::OpenAttention {
                        project: index,
                        thread_id: row.thread_id.clone(),
                    },
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
    let focus = matches!(app.focus, Focus::Projects | Focus::Global) && app.stack.is_empty();
    for (slot, (text, command)) in items.into_iter().enumerate().skip(offset).take(window) {
        let y = rows.start + (slot - offset) as u16;
        if y >= rows.end {
            break;
        }
        let shown = match &command {
            Command::SelectProject(index) if *index == app.project_ix => format!("> {text}"),
            _ => text,
        };
        push(
            out,
            col.x,
            y,
            col.w,
            &shown,
            command,
            (focus && slot == selected, false),
        );
    }
}

fn project_row(snap: &Snapshot, card: &Card) -> String {
    let open = card
        .threads
        .iter()
        .filter(|thread| thread.status == "open")
        .count();
    let attention = snap
        .needs
        .iter()
        .filter(|row| row.project_id == card.id)
        .count();
    let word = match coordinator_state(&card.coordinator) {
        CoordinatorState::Missing => "missing",
        CoordinatorState::Stale => "stale",
        CoordinatorState::Ineligible => "unavailable",
        CoordinatorState::Working => "working",
        CoordinatorState::Blocked => "blocked",
        CoordinatorState::Starting => "starting",
        CoordinatorState::Unknown => "unknown",
        CoordinatorState::Recorded => "socket recorded",
    };
    let availability = if card.availability == "available" {
        String::new()
    } else {
        format!(" {}", card.availability)
    };
    let title = if card.name.is_empty() {
        card.slug.clone()
    } else {
        card.name.clone()
    };
    let slug = if title == card.slug {
        String::new()
    } else {
        format!(" {}", card.slug)
    };
    format!("{title} {word} {open} open {attention} attention{availability}{slug}")
}

fn column_name(app: &App, snap: &Snapshot) -> String {
    match app.card(snap) {
        Some(card) if !card.name.is_empty() => format!("SELECTED / {}", card.name),
        Some(card) => format!("SELECTED / {}", card.slug),
        None => "SELECTED".into(),
    }
}

fn screen_title(_app: &App, _snap: &Snapshot) -> String {
    "HERDR PROJECTS".into()
}

fn place_tabs(
    out: &mut Vec<Placed>,
    app: &App,
    col: &Col,
    y: u16,
    end: u16,
    overview: bool,
    tab: usize,
) -> u16 {
    if y >= end || col.w == 0 {
        return y;
    }
    let focus_tabs = app.focus == Focus::Tabs;
    let mut labels = vec![(
        if overview {
            "[OVERVIEW]".into()
        } else {
            "[Overview]".into()
        },
        Command::ShowOverview,
        overview && focus_tabs,
    )];
    for (index, name) in TABS.iter().enumerate() {
        let current = !overview && index == tab;
        let label = if current {
            format!("[{}]", name.to_ascii_uppercase())
        } else {
            format!("[{name}]")
        };
        labels.push((label, Command::SelectTab(index), current && focus_tabs));
    }
    let widths: Vec<u16> = labels
        .iter()
        .map(|(label, _, _)| label.chars().count() as u16)
        .collect();
    let current = if overview {
        0
    } else {
        tab.saturating_add(1).min(labels.len().saturating_sub(1))
    };
    let limit = col.x.saturating_add(col.w);
    let mut x = col.x;
    if let Some((start, stop)) = tab_window(&widths, current, col.w) {
        if start > 0 {
            x = paint_chip(out, x, y, limit, "[<]", tab_command(start - 1), false);
        }
        for (label, command, selected) in labels.iter().take(stop).skip(start) {
            x = paint_chip(out, x, y, limit, label, command.clone(), *selected);
        }
        if stop < labels.len() {
            paint_chip(out, x, y, limit, "[>]", tab_command(stop), false);
        }
    } else if let Some((label, command, selected)) = labels.get(current) {
        paint_chip(out, x, y, limit, label, command.clone(), *selected);
    }
    y + 1
}

fn tab_command(index: usize) -> Command {
    if index == 0 {
        Command::ShowOverview
    } else {
        Command::SelectTab(index - 1)
    }
}

fn tab_window(widths: &[u16], current: usize, room: u16) -> Option<(usize, usize)> {
    let count = widths.len();
    if count == 0 || room == 0 || current >= count {
        return None;
    }
    if row_span(widths, 0..count, false, false) <= room {
        return Some((0, count));
    }
    let mut best: Option<(usize, usize)> = None;
    for start in 0..=current {
        for stop in (current + 1)..=count {
            let lead = start > 0;
            let trail = stop < count;
            if row_span(widths, start..stop, lead, trail) > room {
                continue;
            }
            let replace = match best {
                None => true,
                Some((old_start, old_stop)) => {
                    let span = stop - start;
                    let old_span = old_stop - old_start;
                    span > old_span || (span == old_span && start < old_start)
                }
            };
            if replace {
                best = Some((start, stop));
            }
        }
    }
    best
}

fn row_span(widths: &[u16], range: std::ops::Range<usize>, lead: bool, trail: bool) -> u16 {
    let mut total = 0u16;
    let mut count = 0u16;
    if lead {
        total = total.saturating_add(3);
        count += 1;
    }
    for width in &widths[range] {
        if count > 0 {
            total = total.saturating_add(1);
        }
        total = total.saturating_add(*width);
        count += 1;
    }
    if trail {
        if count > 0 {
            total = total.saturating_add(1);
        }
        total = total.saturating_add(3);
    }
    total
}

fn paint_chip(
    out: &mut Vec<Placed>,
    x: u16,
    y: u16,
    limit: u16,
    label: &str,
    command: Command,
    selected: bool,
) -> u16 {
    if x >= limit {
        return x;
    }
    let width = (label.chars().count() as u16).min(limit.saturating_sub(x));
    push(out, x, y, width, label, command, (selected, false));
    x.saturating_add(width).saturating_add(1)
}

fn work_lines(app: &App, snap: &Snapshot) -> (Vec<(String, Command, bool)>, usize) {
    let card = app.card(snap);
    let mut lines: Vec<(String, Command, bool)> = Vec::new();
    lines.push(("GOAL".into(), Command::Nothing, false));
    match card {
        None => {
            lines.push(("no project".into(), Command::Nothing, false));
            lines.push(("COORDINATOR".into(), Command::Nothing, false));
            lines.push(("No coordinator recorded.".into(), Command::Nothing, false));
        }
        Some(card) => {
            let goal = if card.goal.is_empty() {
                "(none set)".into()
            } else {
                card.goal.clone()
            };
            lines.push((goal, Command::Nothing, false));
            lines.push(("COORDINATOR".into(), Command::Nothing, false));
            lines.push((
                coordinator_blurb(&card.coordinator),
                Command::Nothing,
                false,
            ));
            if let Some(id) = app.project_id(snap) {
                if let Some(prompt) = app.prompts.get(&id)
                    && let Some((text, label)) = prompt.outgoing.last()
                {
                    let status = label.split(':').next().unwrap_or(label);
                    lines.push((
                        format!("you sent: {text} ({status})"),
                        Command::Nothing,
                        false,
                    ));
                }
                if let Some(notice) = app.notices.get(&id) {
                    lines.push((notice.clone(), Command::Nothing, false));
                }
            }
        }
    }
    let actions = crate::screen::state::coordinator_actions(card);
    let prompt_focused = app.focus == Focus::Prompt && app.stack.is_empty();
    let nav = card.and_then(|card| app.nav.get(&card.id));
    let action_at = nav
        .map(|nav| clamp(nav.work.selected, actions.len()))
        .unwrap_or(0);
    let mut tail_count = actions.len();
    for (index, (text, command)) in actions.into_iter().enumerate() {
        lines.push((text, command, index == action_at));
    }
    if prompt_focused {
        let (word, draft) = prompt_bits(app, snap);
        if word == "Uncertain" {
            lines.push((
                "DELIVERY UNCERTAIN. Herdr did not confirm the prompt. It will not be sent again."
                    .into(),
                Command::Nothing,
                false,
            ));
            tail_count += 1;
        }
        lines.push((format!("{word}: {draft}"), Command::SubmitPrompt, true));
        tail_count += 1;
    }
    (lines, tail_count)
}

fn place_work(
    out: &mut Vec<Placed>,
    app: &App,
    snap: &Snapshot,
    col: &Col,
    rows: std::ops::Range<u16>,
) {
    if rows.start >= rows.end {
        return;
    }
    let (mut lines, tail_count) = work_lines(app, snap);
    let prompt_focused = app.focus == Focus::Prompt && app.stack.is_empty();
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
        paint_lines(
            out,
            col,
            status_end..rows.end,
            tail,
            focus_work || prompt_focused,
        );
    } else {
        paint_lines(out, col, rows, lines, focus_work || prompt_focused);
    }
}

fn coordinator_mark(line: &str) -> String {
    let body = line.trim().trim_start_matches("coordinator:").trim();
    body.split(" pane ")
        .next()
        .unwrap_or(body)
        .trim()
        .to_string()
}

fn health_lines(card: &Card) -> (Vec<String>, Vec<String>) {
    let mut early = Vec::new();
    if let Some(line) = card
        .recovery
        .iter()
        .find(|line| line.starts_with("unresolved"))
    {
        early.push(line.clone());
    }
    if let Some(line) = card
        .operations
        .iter()
        .find(|line| !line.starts_with("operation done"))
    {
        early.push(line.clone());
    }
    let mut rest = Vec::new();
    for line in card.recovery.iter().take(4) {
        if !early.contains(line) {
            rest.push(line.clone());
        }
    }
    for line in card
        .operations
        .iter()
        .filter(|line| !line.starts_with("operation done"))
        .take(2)
    {
        if !early.contains(line) {
            rest.push(line.clone());
        }
    }
    (early, rest)
}

fn coordinator_blurb(line: &str) -> String {
    match coordinator_state(line) {
        CoordinatorState::Missing => "No coordinator recorded.".into(),
        CoordinatorState::Stale => {
            "Stale coordinator. The recorded pane is not a live primary.".into()
        }
        CoordinatorState::Working => "Coordinator is working.".into(),
        CoordinatorState::Blocked => "Coordinator is blocked.".into(),
        CoordinatorState::Starting => "Coordinator is starting.".into(),
        CoordinatorState::Unknown => "Coordinator status is unknown.".into(),
        CoordinatorState::Ineligible => "Coordinator record is not eligible to focus.".into(),
        CoordinatorState::Recorded => {
            "Socket recorded. A socket is not proof the agent is running.".into()
        }
    }
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
    let nav = card.and_then(|card| app.nav.get(&card.id));
    let overview = showing_overview(nav);
    let tab = nav.map(|nav| nav.tab).unwrap_or(0);
    let mut y = rows.start;
    if y < rows.end {
        y = place_tabs(out, app, col, y, rows.end, overview, tab);
    }
    let items = feature_rows(app, snap);
    let pos = card
        .and_then(|card| app.nav.get(&card.id))
        .map(crate::screen::state::tab_list)
        .cloned()
        .unwrap_or_default();
    let selected = clamp(pos.selected, items.len());
    let window = (rows.end.saturating_sub(y)) as usize;
    let offset = visible_offset(pos.offset, selected, items.len(), window.max(1));
    let focus = app.focus == Focus::Overview && app.stack.is_empty();
    let mut last_status = String::new();
    for (slot, (text, command)) in items.into_iter().enumerate().skip(offset).take(window) {
        if y >= rows.end {
            break;
        }
        if !overview
            && tab == 0
            && let Some(thread) = card.and_then(|card| thread_by_selection(card, slot))
            && thread.status != last_status
        {
            push(
                out,
                col.x,
                y,
                col.w,
                &format!("group {}", thread.status),
                Command::Nothing,
                (false, false),
            );
            last_status = thread.status.clone();
            y += 1;
            if y >= rows.end {
                break;
            }
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

pub fn feature_len(app: &App, snap: &Snapshot) -> usize {
    feature_rows(app, snap).len()
}

fn feature_rows(app: &App, snap: &Snapshot) -> Vec<(String, Command)> {
    let Some(card) = app.card(snap) else {
        return vec![("no project".into(), Command::Nothing)];
    };
    if showing_overview(app.nav.get(&card.id)) {
        return summary_rows(app, snap, card);
    }
    match app.nav.get(&card.id).map(|nav| nav.tab).unwrap_or(0) {
        0 => thread_rows_for(card),
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
            |pr| pr_text(&pr.title, &pr.reference),
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

fn thread_rows_for(card: &Card) -> Vec<(String, Command)> {
    let mut rows = Vec::new();
    if card.threads.is_empty() {
        rows.push(("No Threads yet".into(), Command::Nothing));
    } else {
        for index in thread_order(card) {
            if let Some(thread) = card.threads.get(index) {
                rows.push((thread_text(thread), Command::Inspect));
            }
        }
    }
    let start = if card.threads.is_empty() {
        "Start first Thread"
    } else {
        "Start thread"
    };
    rows.push((start.into(), Command::StartThreadForm));
    rows
}

fn summary_rows(app: &App, snap: &Snapshot, card: &Card) -> Vec<(String, Command)> {
    let _ = app;
    let mut rows = Vec::new();
    let open = card
        .threads
        .iter()
        .filter(|thread| thread.status == "open")
        .count();
    let failed = card
        .threads
        .iter()
        .filter(|thread| thread.status == "failed")
        .count();
    let resolved = card
        .threads
        .iter()
        .filter(|thread| thread.status == "resolved")
        .count();
    let project = snap
        .projects
        .iter()
        .position(|item| item.id == card.id)
        .unwrap_or(0);
    rows.push(("WORK".into(), Command::Nothing));
    rows.push((
        format!("{open} open  {failed} failed  {resolved} resolved"),
        Command::Nothing,
    ));
    let (early, recovery) = health_lines(card);
    for line in early {
        rows.push((line, Command::Nothing));
    }
    let needs: Vec<_> = snap
        .needs
        .iter()
        .filter(|row| row.project_id == card.id)
        .take(3)
        .collect();
    if !needs.is_empty() {
        rows.push(("NEEDS ATTENTION".into(), Command::Nothing));
        for row in needs {
            rows.push((
                format!("{} {}", row.thread_id, row.title),
                Command::OpenAttention {
                    project,
                    thread_id: row.thread_id.clone(),
                },
            ));
        }
    }
    let active: Vec<_> = card
        .threads
        .iter()
        .filter(|thread| thread.status == "open" && !thread.pane_id.is_empty())
        .take(3)
        .collect();
    if !active.is_empty() {
        rows.push(("ACTIVE".into(), Command::Nothing));
        for thread in active {
            rows.push((
                thread_line(thread, false),
                Command::OpenAttention {
                    project,
                    thread_id: thread.id.clone(),
                },
            ));
        }
    }
    let reports: Vec<_> = card
        .reports
        .iter()
        .filter(|line| !line.contains("no report file"))
        .take(2)
        .collect();
    if !reports.is_empty() {
        rows.push(("RECENT".into(), Command::Nothing));
        for report in reports {
            let text = report.lines().next().unwrap_or(report);
            rows.push((text.to_string(), Command::Nothing));
        }
    }
    let blurb = coordinator_blurb(&card.coordinator);
    let mark = coordinator_mark(&card.coordinator);
    let mut health = Vec::new();
    if !mark.is_empty() && !blurb.contains(&mark) {
        health.push(mark);
    }
    health.extend(recovery);
    if !health.is_empty() {
        rows.push(("HEALTH".into(), Command::Nothing));
        for line in health {
            rows.push((line, Command::Nothing));
        }
    }
    if card.threads.is_empty() {
        rows.push(("Start first Thread".into(), Command::StartThreadForm));
    }
    rows
}

pub fn thread_order(card: &Card) -> Vec<usize> {
    let mut indexed: Vec<(usize, &str)> = card
        .threads
        .iter()
        .enumerate()
        .map(|(index, thread)| (index, thread.status.as_str()))
        .collect();
    indexed.sort_by(|left, right| left.1.cmp(right.1).then(left.0.cmp(&right.0)));
    indexed.into_iter().map(|(index, _)| index).collect()
}

pub fn thread_by_selection(
    card: &Card,
    selected: usize,
) -> Option<&crate::screen::load::ThreadLine> {
    let order = thread_order(card);
    order
        .get(selected)
        .and_then(|index| card.threads.get(*index))
}

pub fn selected_thread<'a>(
    app: &App,
    snap: &'a Snapshot,
) -> Option<&'a crate::screen::load::ThreadLine> {
    let card = app.card(snap)?;
    let nav = app.nav.get(&card.id)?;
    thread_by_selection(card, nav.threads.selected)
}

pub fn display_index(card: &Card, id: &str) -> Option<usize> {
    thread_order(card).into_iter().position(|index| {
        card.threads
            .get(index)
            .is_some_and(|thread| thread.id == id)
    })
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
    let title = format!("[< {}]", TABS.get(tab).copied().unwrap_or("Threads"));
    let mut y = rows.start;
    if y < rows.end {
        push(
            out,
            col.x,
            y,
            col.w,
            &title,
            Command::Cancel,
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
            let thread = selected_thread(app, snap);
            let head = thread
                .map(thread_text)
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
                    pr.map(|pr| pr_text(&pr.title, &pr.reference))
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
    let narrow = width < 100;
    let offset = if narrow {
        match layer {
            Layer::Picker => app.picker_list.offset,
            Layer::Menu => app.menu_list.offset,
            _ => 0,
        }
    } else {
        0
    };
    for (y, (index, (text, command))) in (1u16..).zip(rows.into_iter().enumerate().skip(offset)) {
        if y >= height {
            break;
        }
        let shown = match (&command, narrow) {
            (Command::SelectProject(index), true) => snap
                .projects
                .get(*index)
                .map(|card| {
                    if card.name.is_empty() {
                        card.slug.clone()
                    } else {
                        card.name.clone()
                    }
                })
                .unwrap_or(text),
            _ => text,
        };
        push(out, x, y, box_w, &shown, command, (index == selected, true));
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
    rows.push(("Advanced settings".into(), Command::ToggleAdvanced));
    rows.push(("Create Project".into(), Command::SubmitNew));
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
    let (label, submit) = if form.allocated.is_some() {
        ("Restart", Command::Restart)
    } else {
        ("Start Thread", Command::SubmitThread)
    };
    rows.push((label.into(), submit));
    rows.push(("Cancel".into(), Command::Cancel));
    if !form.error.is_empty() {
        rows.push((form.error.clone(), Command::Nothing));
    }
    rows
}

fn pr_text(title: &str, reference: &str) -> String {
    format!("{title} recorded reference, not a live check, {reference}")
}

fn thread_text(thread: &crate::screen::load::ThreadLine) -> String {
    thread_line(thread, true)
}

fn thread_line(thread: &crate::screen::load::ThreadLine, full: bool) -> String {
    let mut text = format!("{} {}", thread.id, thread.kind);
    if !thread.harness.is_empty() {
        text.push(' ');
        text.push_str(&thread.harness);
    }
    if !thread.machine.is_empty() {
        text.push(' ');
        text.push_str(&thread.machine);
    }
    if !thread.worktree.is_empty()
        && let Some(name) = std::path::Path::new(&thread.worktree)
            .file_name()
            .and_then(|name| name.to_str())
    {
        text.push(' ');
        text.push_str(name);
    }
    text.push(' ');
    text.push_str(&thread.status);
    text.push(' ');
    text.push_str(&thread.title);
    if thread.unavailable {
        text.push_str(" unavailable");
    }
    if full && !thread.pr.is_empty() {
        text.push_str(" recorded reference ");
        text.push_str(&thread.pr);
        text.push_str(" not a live check");
    }
    if full && !thread.worktree.is_empty() {
        text.push(' ');
        text.push_str(&thread.worktree);
    }
    text
}

pub fn open_worker_thread<'a>(
    app: &App,
    snap: &'a Snapshot,
) -> Option<&'a crate::screen::load::ThreadLine> {
    let card = app.card(snap)?;
    let nav = app.nav.get(&card.id)?;
    if nav.overview {
        let rows = feature_rows(app, snap);
        let index = clamp(nav.overview_pos.selected, rows.len());
        let thread_id = match rows.get(index).map(|(_, command)| command) {
            Some(Command::OpenAttention { thread_id, .. }) => thread_id.as_str(),
            _ => return None,
        };
        return card.threads.iter().find(|thread| thread.id == thread_id);
    }
    if nav.tab == 0 {
        return selected_thread(app, snap);
    }
    None
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
    if width == 0 || text.is_empty() {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 1 {
        return "…".into();
    }
    let keep = width - 1;
    let mut end = text.len();
    for (count, (index, _)) in text.char_indices().enumerate() {
        if count == keep {
            end = index;
            break;
        }
    }
    let head = &text[..end];
    let cut = match head.rfind(char::is_whitespace) {
        Some(space) if space > 0 && head[space..].chars().count() <= 12 => &head[..space],
        _ => head,
    };
    format!("{}…", cut.trim_end())
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
