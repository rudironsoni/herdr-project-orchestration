//! The projects screen. `prefix+a` opens it. `g` then a digit changes the
//! view. Every key runs a CLI command of this binary, so the screen can do
//! nothing the CLI cannot. It redraws on input, and when a list refresh
//! changes the rows.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::spawn;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::{cursor, execute, queue, terminal};

use crate::paths::Ctx;
use crate::profiles::Role;
use crate::project::{self, Project, Status};
use crate::thread::{self, Group, Thread};

const REFRESH: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Overview,
    NeedsYou,
    Work,
    Places,
    Reviews,
    Tasks,
    Inbox,
    Automation,
    More,
    Memory,
    Settings,
}

const VIEWS: [View; 9] = [
    View::Overview,
    View::NeedsYou,
    View::Work,
    View::Places,
    View::Reviews,
    View::Tasks,
    View::Inbox,
    View::Automation,
    View::More,
];

impl View {
    fn name(self) -> &'static str {
        match self {
            View::Overview => "Overview",
            View::NeedsYou => "Needs you",
            View::Work => "Work",
            View::Places => "Places",
            View::Reviews => "Reviews",
            View::Tasks => "Tasks",
            View::Inbox => "Inbox",
            View::Automation => "Automation",
            View::More => "More",
            View::Memory => "Memory",
            View::Settings => "Settings",
        }
    }

    fn from_digit(digit: char) -> Option<View> {
        VIEWS.get(digit.to_digit(10)? as usize - 1).copied()
    }

    fn keys(self) -> &'static str {
        match self {
            View::Overview => "↵ open",
            View::NeedsYou => "↵ jump  i detail  1-9 next  a ack",
            View::Work => {
                "↵ jump  i detail  1-9 next  s stop  r restart  a ack  x resolve  t start  o PR"
            }
            View::Places => "↵ open",
            View::Reviews => "↵ jump  i detail  o PR",
            View::Tasks => "↵ jump  i notes  d delegate  m done  D drop",
            View::Inbox => "↵ detail  a done",
            View::Automation => "↵ toggle  i prompt",
            View::More => "↵ open",
            View::Settings => {
                "↵ edit  n new profile  d delete profile  Y yolo  p pause/resume  A archive  X delete"
            }
            View::Memory => "↵ read",
        }
    }

    fn nested(self) -> bool {
        matches!(self, View::Memory | View::Settings)
    }
}

// ---------------------------------------------------------------- data

#[derive(Debug, Clone)]
pub struct ThreadRow {
    pub slug: String,
    pub socket: String,
    pub thread: Thread,
    pub group: Group,
    pub next: Vec<String>,
    pub pr_facts: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub slug: String,
    pub list: String,
    pub title: String,
    pub owner: String,
    pub thread: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct Row {
    /// A group or project heading: not selectable.
    pub header: bool,
    pub text: String,
    pub color: Option<Color>,
    pub kind: RowKind,
}

#[derive(Debug, Clone)]
pub enum RowKind {
    None,
    Thread(Box<ThreadRow>),
    Task(TaskRow),
    Inbox {
        slug: String,
        id: String,
        body: String,
    },
    Routine {
        slug: String,
        name: String,
        prompt: String,
    },
    /// A setting; `slug` is empty for a user-wide profile setting.
    Setting {
        slug: String,
        key: String,
        value: String,
    },
    /// A safety setting of a project, or of all projects (`slug` None).
    Safety {
        slug: Option<String>,
        key: String,
        value: String,
    },
    Profile {
        name: String,
        builtin: bool,
    },
    Project {
        slug: String,
    },
    Memory {
        path: PathBuf,
    },
    Coordinator {
        slug: String,
    },
    Place {
        slug: String,
        cwd: String,
        unresolved: bool,
        socket: String,
        machine: String,
        pane: String,
        repository_id: Option<String>,
    },
    MoreMemory,
    MoreSettings,
}

/// Parses TASKS.md (see `tasks::parse`) into popup rows.
pub fn parse_tasks(slug: &str, text: &str) -> Vec<TaskRow> {
    crate::tasks::parse(text)
        .into_iter()
        .map(|t| TaskRow {
            slug: slug.to_string(),
            list: t.list,
            title: t.title,
            owner: t.owner.to_string(),
            thread: t.thread,
            description: t.description,
        })
        .collect()
}

/// `PR #4 · approved · checks ✓ · 2 comments`, from the ticker's last poll.
pub fn pr_facts(thread: &Thread, summary: Option<&crate::pr::Summary>) -> String {
    let Some(number) = thread
        .pr
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty() && !thread.pr.is_empty())
    else {
        return String::new();
    };
    let mut parts = vec![format!("PR #{number}")];
    if let Some(s) = summary {
        let state = s.state.to_lowercase();
        if state != "open" && !state.is_empty() {
            parts.push(state);
        }
        match s.review_decision.as_str() {
            "APPROVED" => parts.push("approved".into()),
            "CHANGES_REQUESTED" => parts.push("changes requested".into()),
            _ => {}
        }
        parts.push(if s.failing_checks.is_empty() {
            "checks ✓".into()
        } else {
            format!("checks ✗ {}", s.failing_checks.len())
        });
        if s.comment_count > 0 {
            parts.push(format!(
                "{} comment{}",
                s.comment_count,
                if s.comment_count == 1 { "" } else { "s" }
            ));
        }
    }
    parts.join(" · ")
}

fn projects_in_scope(root: &Path, scope: Option<&str>, show_archived: bool) -> Vec<Project> {
    project::list_slugs(root)
        .into_iter()
        .filter(|s| scope.is_none_or(|scope| scope == s))
        .filter_map(|s| Project::load(root, &s).ok())
        .filter(|p| show_archived || p.status() != Status::Archived)
        .collect()
}

/// One row of the project picker; `slug` is `None` for "All projects".
#[derive(Debug, Clone, PartialEq)]
pub struct PickerRow {
    pub slug: Option<String>,
    pub name: String,
    pub status: String,
}

/// The rows `P` and `/` offer: "All projects", then every listed project.
pub fn picker_rows(root: &Path) -> Vec<PickerRow> {
    let mut rows = vec![PickerRow {
        slug: None,
        name: "All projects".into(),
        status: summary(root),
    }];
    for project in projects_in_scope(root, None, false) {
        let name = project
            .read_project_md()
            .map(|(s, _)| project::display_name(&s.name, &project.slug))
            .unwrap_or_else(|_| project.slug.clone());
        let status = crate::sidebar::project_line(
            &crate::sidebar::recorded_groups(&project),
            project.status() == Status::Paused,
        );
        rows.push(PickerRow {
            slug: Some(project.slug.clone()),
            name,
            status,
        });
    }
    rows
}

/// The project picker: ↑↓ move (wrapping), ↵ switches the scope, `/` filters
/// on name and slug, esc clears the filter and then closes.
#[derive(Debug, Clone)]
pub struct Picker {
    pub rows: Vec<PickerRow>,
    /// The typed filter while filtering.
    pub filter: Option<String>,
    /// An index into `visible()`.
    pub selected: usize,
}

#[derive(Debug, PartialEq)]
pub enum PickerOutcome {
    Stay,
    Close,
    Pick(Option<String>),
}

impl Picker {
    /// Opens on the current scope; `filtering` starts with an empty filter.
    pub fn new(rows: Vec<PickerRow>, scope: Option<&str>, filtering: bool) -> Picker {
        let selected = rows
            .iter()
            .position(|r| r.slug.as_deref() == scope)
            .unwrap_or(0);
        Picker {
            rows,
            filter: filtering.then(String::new),
            selected,
        }
    }

    pub fn visible(&self) -> Vec<&PickerRow> {
        let needle = self.filter.as_deref().unwrap_or("").to_lowercase();
        self.rows
            .iter()
            .filter(|r| {
                r.name.to_lowercase().contains(&needle)
                    || r.slug.as_deref().is_some_and(|s| s.contains(&needle))
            })
            .collect()
    }

    fn step(&mut self, forward: bool) {
        let n = self.visible().len();
        if n > 0 {
            self.selected = if forward {
                (self.selected + 1) % n
            } else {
                (self.selected + n - 1) % n
            };
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> PickerOutcome {
        let filtering = self.filter.is_some();
        match key.code {
            KeyCode::Up => self.step(false),
            KeyCode::Down => self.step(true),
            KeyCode::Char('k') if !filtering => self.step(false),
            KeyCode::Char('j') if !filtering => self.step(true),
            KeyCode::Enter => {
                if let Some(row) = self.visible().get(self.selected) {
                    return PickerOutcome::Pick(row.slug.clone());
                }
            }
            KeyCode::Esc => {
                // A typed filter is cleared first, keeping the highlighted row.
                if self.filter.as_ref().is_some_and(|f| !f.is_empty()) {
                    let highlighted = self.visible().get(self.selected).map(|r| r.slug.clone());
                    self.filter = None;
                    self.selected = highlighted
                        .and_then(|slug| self.rows.iter().position(|r| r.slug == slug))
                        .unwrap_or(0);
                } else {
                    return PickerOutcome::Close;
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return PickerOutcome::Close;
            }
            KeyCode::Char('/') if !filtering => {
                self.filter = Some(String::new());
            }
            KeyCode::Backspace if filtering => {
                if let Some(filter) = &mut self.filter {
                    filter.pop();
                }
                self.selected = 0;
            }
            KeyCode::Char(c) if filtering && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(filter) = &mut self.filter {
                    filter.push(c);
                }
                self.selected = 0;
            }
            _ => {}
        }
        PickerOutcome::Stay
    }
}

pub fn thread_rows(root: &Path, scope: Option<&str>) -> Vec<ThreadRow> {
    let mut rows = Vec::new();
    for project in projects_in_scope(root, scope, false) {
        let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
        let state = crate::steps::load_state(&project);
        for t in thread::list(&project) {
            let group = if t.status == thread::Status::Resolved {
                Group::Resolved
            } else {
                Group::from_token(&t.last_group).unwrap_or(Group::Working)
            };
            rows.push(ThreadRow {
                slug: project.slug.clone(),
                socket: socket.clone(),
                next: thread::all_next(&project, &t.id),
                pr_facts: pr_facts(&t, state.prs.get(&t.id)),
                group,
                thread: t,
            });
        }
    }
    rows
}

fn group_color(group: Group) -> Option<Color> {
    match group {
        Group::WaitingOnYou => Some(Color::Red),
        Group::ReadyForReview => Some(Color::Yellow),
        Group::Landing => Some(Color::Green),
        Group::Resolved => Some(Color::DarkGrey),
        _ => None,
    }
}

/// A task's notes as a detail screen.
fn task_detail(task: &TaskRow) -> Mode {
    let mut lines = vec![
        format!(
            "{} · {}",
            task.list,
            if task.owner.is_empty() {
                "no owner"
            } else {
                &task.owner
            }
        ),
        String::new(),
    ];
    if task.description.trim().is_empty() {
        lines.push("(no notes; ask the coordinator to add some)".into());
    } else {
        lines.extend(task.description.lines().map(|l| format!("  {l}")));
    }
    Mode::Detail {
        title: task.title.clone(),
        lines,
        files: Vec::new(),
        selected: 0,
        scroll: 0,
    }
}

fn header(text: impl Into<String>) -> Row {
    Row {
        header: true,
        text: text.into(),
        color: None,
        kind: RowKind::None,
    }
}

fn pr_number(thread: &Thread) -> Option<&str> {
    let number = thread.pr.rsplit('/').next()?;
    (!thread.pr.is_empty() && !number.is_empty()).then_some(number)
}

fn thread_word(thread: &Thread, group: Group) -> &'static str {
    if thread.status == thread::Status::Failed {
        "failed"
    } else {
        crate::sidebar::word(group)
    }
}

fn thread_line(r: &ThreadRow, with_project: bool) -> String {
    let t = &r.thread;
    let mut parts = vec![t.id.clone(), thread_word(t, r.group).to_string()];
    if t.status != thread::Status::Failed && t.last_state == "blocked" {
        parts.push("blocked".into());
    }
    if let Some(number) = pr_number(t) {
        parts.push(format!("PR #{number}"));
    }
    if !r.next.is_empty() {
        parts.push(format!("next: {}", r.next.len()));
    }
    if !t.title.is_empty() {
        parts.push(t.title.clone());
    }
    if !t.state_line.is_empty() {
        parts.push(t.state_line.clone());
    }
    if !t.activity.is_empty() && r.group != Group::Resolved {
        parts.push(t.activity.clone());
    }
    if !r.pr_facts.is_empty() {
        parts.push(r.pr_facts.clone());
    }
    if !t.machine.is_empty() {
        parts.push(format!("on {}", t.machine));
    }
    if !t.agent.is_empty() {
        parts.push(t.agent.clone());
    }
    let line = parts.join(" · ");
    if with_project {
        format!("{} · {line}", r.slug)
    } else {
        line
    }
}

fn person_needs(row: &ThreadRow) -> bool {
    row.group == Group::WaitingOnYou && row.thread.status != thread::Status::Failed
}

fn in_review(row: &ThreadRow) -> bool {
    matches!(row.group, Group::ReadyForReview | Group::Landing)
}

fn push_threads(rows: &mut Vec<Row>, threads: &[ThreadRow], with_project: bool) {
    if threads.is_empty() {
        rows.push(header("no threads"));
        return;
    }
    for row in threads {
        let failed = row.thread.status == thread::Status::Failed;
        rows.push(Row {
            header: false,
            text: format!("  {}", thread_line(row, with_project)),
            color: if failed {
                Some(Color::Red)
            } else {
                group_color(row.group)
            },
            kind: RowKind::Thread(Box::new(row.clone())),
        });
    }
}

fn coordinator_line(project: &Project) -> String {
    let alive = crate::coordinator::reachable_coordinator(project).is_some();
    let session = (|| {
        let store = crate::store::Store::open(&project.root).ok()?;
        let row = store.project_by_slug(&project.slug).ok()??;
        store.primary_session(&row.id).ok()?
    })();
    let mut bits = vec!["coordinator".to_string()];
    if session.as_ref().is_some_and(|session| session.stale) {
        bits.push("stale".into());
    } else if alive {
        bits.push("alive".into());
    } else {
        bits.push("unavailable".into());
    }
    if let (Some(session), Ok((settings, _))) = (&session, project.read_project_md())
        && !session.profile.is_empty()
    {
        bits.push(
            if session.profile == settings.coordinator_profile {
                "profile match"
            } else {
                "profile differs"
            }
            .into(),
        );
    }
    bits.join(" · ")
}

fn pane_for_cwd(root: &Path, slug: &str, cwd: &str) -> (String, String, String) {
    thread_rows(root, Some(slug))
        .into_iter()
        .find(|row| {
            !row.thread.pane_id.is_empty()
                && (row.thread.cwd == cwd || row.thread.worktree_path == cwd)
        })
        .map(|row| (row.socket, row.thread.machine, row.thread.pane_id))
        .unwrap_or_default()
}

fn place_row(
    project: &Project,
    cwd: &str,
    branch: &str,
    ownership: &str,
    availability: &str,
    unresolved: bool,
    repository_id: Option<String>,
) -> Row {
    let (socket, machine, pane) = pane_for_cwd(&project.root, &project.slug, cwd);
    let mut text = format!("  {cwd}");
    if !branch.is_empty() {
        text.push_str(&format!(" · {branch}"));
    }
    if !ownership.is_empty() {
        text.push_str(&format!(" · {ownership}"));
    }
    if !availability.is_empty() {
        text.push_str(&format!(" · {availability}"));
    }
    if unresolved {
        text.push_str(" · unresolved");
    }
    Row {
        header: false,
        text,
        color: if unresolved { Some(Color::Red) } else { None },
        kind: RowKind::Place {
            slug: project.slug.clone(),
            cwd: cwd.to_string(),
            unresolved,
            socket,
            machine,
            pane,
            repository_id,
        },
    }
}

fn places_for(rows: &mut Vec<Row>, project: &Project) {
    let loaded = crate::store::Store::open(&project.root)
        .ok()
        .and_then(|store| {
            let row = store.project_by_slug(&project.slug).ok()??;
            let repos = store.repos(&row.id).ok()?;
            let workspaces = store.list_workspaces(&row.id).ok()?;
            Some((repos, workspaces))
        });
    if let Some((repos, workspaces)) = loaded
        && (!repos.is_empty() || !workspaces.is_empty())
    {
        for repo in &repos {
            let mut label = repo.path.clone();
            if !repo.machine_label.is_empty() {
                label.push_str(&format!(" · {}", repo.machine_label));
            }
            if repo.removed {
                label.push_str(" · removed");
            }
            rows.push(header(label));
            for workspace in workspaces
                .iter()
                .filter(|workspace| workspace.repository_id.as_deref() == Some(repo.id.as_str()))
            {
                rows.push(place_row(
                    project,
                    &workspace.cwd,
                    &workspace.branch,
                    &workspace.ownership,
                    &workspace.availability,
                    workspace.environment_state != "resolved",
                    workspace.repository_id.clone(),
                ));
            }
        }
        let nulls: Vec<_> = workspaces
            .iter()
            .filter(|workspace| workspace.repository_id.is_none())
            .collect();
        if !nulls.is_empty() {
            rows.push(header("Project directory"));
            for workspace in nulls {
                rows.push(place_row(
                    project,
                    &workspace.cwd,
                    &workspace.branch,
                    &workspace.ownership,
                    &workspace.availability,
                    workspace.environment_state != "resolved",
                    None,
                ));
            }
        }
        return;
    }
    if let Ok((settings, _)) = project.read_project_md() {
        for repo in &settings.repos {
            let machine = repo.machine.as_deref().unwrap_or("");
            let label = if machine.is_empty() {
                repo.path.clone()
            } else {
                format!("{} · {machine}", repo.path)
            };
            rows.push(header(label));
        }
    }
    rows.push(header("Project directory"));
    let cwd = project.dir().to_string_lossy().into_owned();
    rows.push(place_row(project, &cwd, "", "legacy", "", false, None));
}

/// The rows of a view, with headings.
pub fn build(ctx: &Ctx, view: View, scope: Option<&str>) -> Vec<Row> {
    let root = ctx.root.as_path();
    let mut rows = Vec::new();
    match view {
        View::Overview => {
            let threads = thread_rows(root, scope);
            let open = threads
                .iter()
                .filter(|row| row.thread.status != thread::Status::Resolved)
                .count();
            let needs = threads.iter().filter(|row| person_needs(row)).count();
            let reviews = threads.iter().filter(|row| in_review(row)).count();
            rows.push(header(format!(
                "{open} open · {needs} needs you · {reviews} reviews"
            )));
            for project in projects_in_scope(root, scope, false) {
                rows.push(Row {
                    header: false,
                    text: format!("  {}", coordinator_line(&project)),
                    color: None,
                    kind: RowKind::Coordinator {
                        slug: project.slug.clone(),
                    },
                });
            }
        }
        View::NeedsYou => {
            let mut threads: Vec<ThreadRow> = thread_rows(root, scope)
                .into_iter()
                .filter(person_needs)
                .collect();
            threads.sort_by(|a, b| a.thread.id.cmp(&b.thread.id));
            push_threads(&mut rows, &threads, scope.is_none());
        }
        View::Work => {
            let mut threads: Vec<ThreadRow> = thread_rows(root, scope)
                .into_iter()
                .filter(|row| row.thread.status != thread::Status::Resolved)
                .collect();
            threads.sort_by_key(|row| (row.group.rank(), row.thread.id.clone()));
            push_threads(&mut rows, &threads, scope.is_none());
        }
        View::Reviews => {
            let mut threads: Vec<ThreadRow> = thread_rows(root, scope)
                .into_iter()
                .filter(in_review)
                .collect();
            threads.sort_by_key(|row| (row.group.rank(), row.thread.id.clone()));
            push_threads(&mut rows, &threads, scope.is_none());
        }
        View::Places => {
            for project in projects_in_scope(root, scope, false) {
                if scope.is_none() {
                    rows.push(header(project.slug.clone()));
                }
                places_for(&mut rows, &project);
            }
            if rows.is_empty() {
                rows.push(header("no places"));
            }
        }
        View::More => {
            rows.push(Row {
                header: false,
                text: "  Memory".into(),
                color: None,
                kind: RowKind::MoreMemory,
            });
            rows.push(Row {
                header: false,
                text: "  Settings".into(),
                color: None,
                kind: RowKind::MoreSettings,
            });
        }
        View::Tasks => {
            for project in projects_in_scope(root, scope, false) {
                let text =
                    std::fs::read_to_string(project.dir().join("TASKS.md")).unwrap_or_default();
                let tasks = parse_tasks(&project.slug, &text);
                let mut list = None;
                for task in tasks {
                    if list.as_ref() != Some(&task.list) {
                        list = Some(task.list.clone());
                        rows.push(header(if scope.is_none() {
                            format!("{} · {}", project.slug, task.list)
                        } else {
                            task.list.clone()
                        }));
                    }
                    let mut owner = if task.owner.is_empty() {
                        String::new()
                    } else {
                        format!("  ({})", task.owner)
                    };
                    if let Some(thread) = &task.thread {
                        owner.push_str(&format!(" · {thread}"));
                    }
                    let notes = if task.description.trim().is_empty() {
                        ""
                    } else {
                        "  ≡"
                    };
                    rows.push(Row {
                        header: false,
                        text: format!("  {}{owner}{notes}", task.title),
                        color: None,
                        kind: RowKind::Task(task),
                    });
                }
            }
            if rows.is_empty() {
                rows.push(header("no tasks; ask the coordinator to add one"));
            }
        }
        View::Inbox => {
            for project in projects_in_scope(root, scope, false) {
                for item in crate::inbox::unhandled(&project) {
                    let prefix = if scope.is_none() {
                        format!("{} · ", project.slug)
                    } else {
                        String::new()
                    };
                    let body = format!("{}\n\n{}", item.summary, item.body);
                    rows.push(Row {
                        header: false,
                        text: format!(
                            "{prefix}{} · {} · {}",
                            item.kind, item.subject, item.summary
                        ),
                        color: None,
                        kind: RowKind::Inbox {
                            slug: project.slug.clone(),
                            id: item.id,
                            body,
                        },
                    });
                }
            }
            if rows.is_empty() {
                rows.push(header("inbox is empty"));
            }
        }
        View::Automation => {
            for project in projects_in_scope(root, scope, false) {
                let (routines, broken) = crate::routine::load_all(&project);
                let state = crate::steps::load_state(&project);
                let now = jiff::Zoned::now();
                for r in routines {
                    let last =
                        match crate::routine::when_text(&r, state.routines.get(&r.name), &now) {
                            text if text.is_empty() => String::new(),
                            text => format!(" · {text}"),
                        };
                    let prefix = if scope.is_none() {
                        format!("{} · ", project.slug)
                    } else {
                        String::new()
                    };
                    let when = if r.schedule_text.is_empty() {
                        "on pr".to_string()
                    } else {
                        r.schedule_text.clone()
                    };
                    rows.push(Row {
                        header: false,
                        text: format!(
                            "{prefix}{} · {when} · {}{last}",
                            r.name,
                            if r.enabled { "enabled" } else { "disabled" }
                        ),
                        color: if r.enabled {
                            None
                        } else {
                            Some(Color::DarkGrey)
                        },
                        kind: RowKind::Routine {
                            slug: project.slug.clone(),
                            name: r.name.clone(),
                            prompt: r.prompt.clone(),
                        },
                    });
                }
                for b in broken {
                    rows.push(Row {
                        header: false,
                        text: format!("{} · config error: {}", b.file, b.error),
                        color: Some(Color::Red),
                        kind: RowKind::None,
                    });
                }
            }
            if rows.is_empty() {
                rows.push(header("no routines; ask the coordinator for one"));
            }
        }
        View::Settings => match scope {
            None => {
                profile_rows(ctx, &mut rows);
                rows.push(header("projects (↵ opens a project's settings)"));
                for project in projects_in_scope(root, None, true) {
                    let (settings, _) = project.read_project_md().unwrap_or_default_settings();
                    rows.push(Row {
                        header: false,
                        text: format!(
                            "{} · {} · {}",
                            project.slug,
                            project::display_name(&settings.name, &project.slug),
                            project.status()
                        ),
                        color: None,
                        kind: RowKind::Project {
                            slug: project.slug.clone(),
                        },
                    });
                }
            }
            Some(slug) => {
                if let Ok(project) = Project::load(root, slug) {
                    let (s, _) = project.read_project_md().unwrap_or_default_settings();
                    rows.push(header(format!(
                        "{} · {}",
                        project::display_name(&s.name, slug),
                        project.status()
                    )));
                    let values = [
                        ("name", s.name.clone()),
                        ("goal", s.goal.clone()),
                        ("coordinator_profile", s.coordinator_profile.clone()),
                        ("thread_profile", s.thread_profile.clone()),
                        (
                            "coordinator_profiles",
                            allowed_text(ctx, Some(&project), Role::Coordinator),
                        ),
                        (
                            "thread_profiles",
                            allowed_text(ctx, Some(&project), Role::Thread),
                        ),
                        ("max_parallel_threads", s.max_parallel_threads.to_string()),
                        ("auto_resolve_days", s.auto_resolve_days.to_string()),
                        ("nudge", s.nudge.to_string()),
                        ("mute", s.mute.to_string()),
                        ("repos.add", crate::settings::repos_text(&s)),
                        ("repos.remove", crate::settings::repos_text(&s)),
                    ];
                    for (key, value) in values {
                        let label = match key {
                            "repos.add" => "repos (↵ add)".to_string(),
                            "repos.remove" => "repos (↵ remove)".to_string(),
                            k => k.to_string(),
                        };
                        rows.push(Row {
                            header: false,
                            text: format!("  {label:<22} {value}"),
                            color: None,
                            kind: RowKind::Setting {
                                slug: slug.to_string(),
                                key: key.to_string(),
                                value,
                            },
                        });
                    }
                }
            }
        },
        View::Memory => {
            for project in projects_in_scope(root, scope, false) {
                rows.push(header(format!(
                    "{} · MEMORY.md (read only; change it by asking the coordinator)",
                    project.slug
                )));
                rows.push(Row {
                    header: false,
                    text: "  MEMORY.md".into(),
                    color: None,
                    kind: RowKind::Memory {
                        path: project.dir().join("MEMORY.md"),
                    },
                });
                let mut files: Vec<PathBuf> = std::fs::read_dir(project.dir().join("memory"))
                    .map(|e| {
                        e.flatten()
                            .map(|e| e.path())
                            .filter(|p| p.extension().is_some_and(|x| x == "md"))
                            .collect()
                    })
                    .unwrap_or_default();
                files.sort();
                for path in files {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    rows.push(Row {
                        header: false,
                        text: format!("  memory/{name}"),
                        color: None,
                        kind: RowKind::Memory { path },
                    });
                }
            }
        }
    }
    rows
}

/// The settings tab's safety block for `scope`, or the all-projects defaults.
pub fn safety_rows(ctx: &Ctx, scope: Option<&str>) -> Vec<Row> {
    let target = match scope {
        None => crate::safety::Target::Global,
        Some(slug) => match Project::load(&ctx.root, slug) {
            Ok(project) => crate::safety::Target::Project(project),
            Err(_) => return Vec::new(),
        },
    };
    let mut rows = vec![header(match scope {
        None => {
            "safety · all projects (yours; no agent can change it; running agents keep theirs until restarted)"
        }
        Some(_) => {
            "safety (yours; no agent can change it; running agents keep theirs until restarted)"
        }
    })];
    match crate::safety::rows(&ctx.config_dir, &target) {
        Ok(list) => {
            for r in list {
                let label = if r.key == "yolo" {
                    "yolo mode (Y)"
                } else {
                    r.key
                };
                let yolo_on = r.key == "yolo" && r.value == "on";
                rows.push(Row {
                    header: false,
                    text: format!("  {label:<22} {}  · {}", r.text(), r.source),
                    color: yolo_on.then_some(Color::Yellow),
                    kind: RowKind::Safety {
                        slug: scope.map(str::to_string),
                        key: r.key.to_string(),
                        value: r.value,
                    },
                });
            }
        }
        Err(error) => rows.push(Row {
            header: false,
            text: format!("  config error: {error:#}"),
            color: Some(Color::Red),
            kind: RowKind::None,
        }),
    }
    rows
}

/// The user-wide profile rows: each profile, then the defaults for new
/// projects and the allow-lists for projects without their own.
fn profile_rows(ctx: &Ctx, rows: &mut Vec<Row>) {
    let config = match crate::profiles::load(&ctx.config_dir) {
        Ok(config) => config,
        Err(error) => {
            rows.push(Row {
                header: false,
                text: format!("config error: {error:#}"),
                color: Some(Color::Red),
                kind: RowKind::None,
            });
            return;
        }
    };
    rows.push(header(
        "profiles (n new · ↵ edit · d delete; a built-in is replaced by editing it)",
    ));
    for p in config.listed(&crate::profiles::detect(ctx.env)) {
        let text = format!(
            "  {:<14} {}{}",
            p.name,
            p.summary(),
            if p.builtin { "  (built-in)" } else { "" }
        );
        rows.push(Row {
            header: false,
            text,
            color: None,
            kind: RowKind::Profile {
                name: p.name.clone(),
                builtin: p.builtin,
            },
        });
    }
    let values = [
        (
            "thread_profile",
            config.new_project_default(Role::Thread),
            "thread_profile (new projects)",
        ),
        (
            "coordinator_profile",
            config.new_project_default(Role::Coordinator),
            "coordinator_profile (new projects)",
        ),
        (
            "thread_profiles",
            allowed_text(ctx, None, Role::Thread),
            "thread_profiles (all projects)",
        ),
        (
            "coordinator_profiles",
            allowed_text(ctx, None, Role::Coordinator),
            "coordinator_profiles (all projects)",
        ),
    ];
    for (key, value, label) in values {
        rows.push(Row {
            header: false,
            text: format!("  {label:<36} {value}"),
            color: None,
            kind: RowKind::Setting {
                slug: String::new(),
                key: key.into(),
                value,
            },
        });
    }
}

/// A role's allow-list as the settings rows show it.
fn allowed_text(ctx: &Ctx, project: Option<&Project>, role: Role) -> String {
    let config = crate::profiles::load(&ctx.config_dir).unwrap_or_default();
    let safety = project
        .and_then(|p| p.safety(&ctx.config_dir).ok())
        .unwrap_or_default();
    match config.allowed(&safety, role) {
        None => "every profile".into(),
        Some(list) if list.is_empty() => "none".into(),
        Some(list) => list.join(", "),
    }
}

/// One field of the profile form: free text, or a choice cycled with ←→.
#[derive(Debug, Clone)]
struct Field {
    label: &'static str,
    value: String,
    options: Vec<String>,
}

/// The effort choices for a harness: its default, then its own values.
fn effort_options(agent: &str) -> Vec<String> {
    std::iter::once(String::new())
        .chain(
            crate::profiles::effort_values(agent)
                .unwrap_or_default()
                .iter()
                .map(|v| v.to_string()),
        )
        .collect()
}

trait OrDefault {
    fn unwrap_or_default_settings(self) -> (project::Settings, String);
}

impl OrDefault for Result<(project::Settings, String)> {
    fn unwrap_or_default_settings(self) -> (project::Settings, String) {
        self.unwrap_or_default()
    }
}

/// The header summary: `3 projects · 2 need you`.
pub fn summary(root: &Path) -> String {
    let projects = projects_in_scope(root, None, false);
    let need: usize = projects
        .iter()
        .map(|p| {
            crate::sidebar::recorded_groups(p)
                .into_iter()
                .filter(|g| crate::sidebar::needs_you(*g))
                .count()
        })
        .sum();
    format!(
        "{} project{} · {need} need you",
        projects.len(),
        if projects.len() == 1 { "" } else { "s" }
    )
}

// ---------------------------------------------------------------- the loop

enum Mode {
    List,
    /// A scrollable text; `files` are selectable lines that open with ↵.
    Detail {
        title: String,
        lines: Vec<String>,
        files: Vec<PathBuf>,
        selected: usize,
        scroll: usize,
    },
    Confirm {
        question: String,
        action: Vec<String>,
        lines: Vec<String>,
    },
    Edit {
        label: String,
        buffer: String,
        action: Vec<String>,
    },
    Pick {
        label: String,
        options: Vec<String>,
        selected: usize,
        action: Vec<String>,
    },
    /// Several choices at once: space toggles, ↵ runs `action` with the
    /// checked options appended (`--all` when the first, "every profile", is).
    Toggle {
        label: String,
        options: Vec<(String, bool)>,
        selected: usize,
        action: Vec<String>,
    },
    /// The profile form: name, harness, model, effort, arguments, description.
    Form {
        title: String,
        fields: Vec<Field>,
        selected: usize,
        editing: bool,
    },
    /// The project picker (`P`, or `/` straight into its filter).
    Projects(Picker),
    /// `t` on Work: kind, then title and task, or a pane for adopted.
    Start {
        slug: String,
        phase: StartPhase,
        kind: String,
        title: String,
        repo: String,
        buffer: String,
        options: Vec<String>,
        selected: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StartPhase {
    Kind,
    Repo,
    Title,
    Task,
    Pane,
}

impl StartPhase {
    fn label(self) -> &'static str {
        match self {
            StartPhase::Kind => "thread kind",
            StartPhase::Repo => "repo",
            StartPhase::Title => "title",
            StartPhase::Task => "task",
            StartPhase::Pane => "pane",
        }
    }
}

pub struct Popup<'a> {
    ctx: &'a Ctx<'a>,
    scope: Option<String>,
    view: View,
    selected: usize,
    rows: Vec<Row>,
    mode: Mode,
    message: String,
    workspace: String,
    quit: bool,
    leader: bool,
    hide_projects: bool,
    inspect: bool,
    /// A pane to focus once the popup has closed: (socket, machine, pane).
    jump: Option<(String, String, String)>,
}

impl<'a> Popup<'a> {
    pub fn new(ctx: &'a Ctx<'a>, scope: Option<String>, workspace: String) -> Self {
        let mut popup = Popup {
            ctx,
            scope,
            view: View::Overview,
            selected: 0,
            rows: Vec::new(),
            mode: Mode::List,
            message: String::new(),
            workspace,
            quit: false,
            leader: false,
            hide_projects: false,
            inspect: false,
            jump: None,
        };
        popup.reload();
        popup
    }

    fn reload(&mut self) {
        let keep = self.rows.get(self.selected).and_then(row_identity);
        self.rows = build(self.ctx, self.view, self.scope.as_deref());
        if self.view == View::Settings {
            self.rows
                .extend(safety_rows(self.ctx, self.scope.as_deref()));
        }
        if let Some(keep) = keep
            && let Some(index) = self
                .rows
                .iter()
                .position(|row| row_identity(row).as_ref() == Some(&keep))
        {
            self.selected = index;
            return;
        }
        if self.rows.get(self.selected).is_none_or(|r| r.header) {
            self.selected = self
                .rows
                .iter()
                .position(|r| !r.header)
                .unwrap_or(0)
                .max(self.selected.min(self.rows.len().saturating_sub(1)));
            if self.rows.get(self.selected).is_some_and(|r| r.header) {
                self.selected = self.rows.iter().position(|r| !r.header).unwrap_or(0);
            }
        }
    }

    fn set_view(&mut self, view: View) {
        self.view = view;
        self.leader = false;
        self.inspect = false;
        self.selected = 0;
        self.reload();
    }

    fn current(&self) -> Option<&Row> {
        self.rows.get(self.selected).filter(|r| !r.header)
    }

    fn move_by(&mut self, delta: isize) {
        let n = self.rows.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.selected as isize;
        for _ in 0..n {
            i = (i + delta).clamp(0, n - 1);
            if !self.rows[i as usize].header {
                self.selected = i as usize;
                return;
            }
            if i == 0 || i == n - 1 {
                break;
            }
        }
    }

    /// `true` when the event changed what is on screen. A move does not.
    fn mouse(&mut self, event: MouseEvent) -> bool {
        if !matches!(self.mode, Mode::List) {
            return false;
        }
        let before = self.list_view();
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.list_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
            }
            MouseEventKind::ScrollDown => {
                self.list_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.click(event.column as usize, event.row as usize);
            }
            _ => return false,
        }
        self.list_view() != before
    }

    fn list_view(
        &self,
    ) -> (
        usize,
        usize,
        &'static str,
        String,
        String,
        Vec<(bool, String)>,
    ) {
        (
            VIEWS
                .iter()
                .position(|view| *view == self.view)
                .unwrap_or(9),
            self.selected,
            self.mode_tag(),
            self.message.clone(),
            summary(&self.ctx.root),
            self.rows
                .iter()
                .map(|row| (row.header, row.text.clone()))
                .collect(),
        )
    }

    fn mode_tag(&self) -> &'static str {
        match self.mode {
            Mode::List => "list",
            Mode::Detail { .. } => "detail",
            Mode::Confirm { .. } => "confirm",
            Mode::Edit { .. } => "edit",
            Mode::Pick { .. } => "pick",
            Mode::Toggle { .. } => "toggle",
            Mode::Form { .. } => "form",
            Mode::Projects(_) => "projects",
            Mode::Start { .. } => "start",
        }
    }

    fn refresh_list(&mut self) -> bool {
        let before = self.list_view();
        self.reload();
        self.list_view() != before
    }

    fn click(&mut self, column: usize, row: usize) {
        if self.inspect {
            return;
        }
        let (width, height) = screen_size();
        if row == 0 || column >= width {
            return;
        }
        let (body_top, body_height) = body_window(height);
        if row < body_top || row >= body_top + body_height {
            return;
        }
        let (project_w, body_w, _) = column_widths(width, self.hide_projects);
        if project_w > 0 && column < project_w {
            let projects = picker_rows(&self.ctx.root);
            if let Some(project) = projects.get(row - body_top) {
                self.scope = project.slug.clone();
                self.selected = 0;
                self.reload();
            }
            return;
        }
        if column < project_w || column >= project_w + body_w {
            return;
        }
        let index = self.list_start(body_height) + (row - body_top);
        if self.rows.get(index).is_none_or(|target| target.header) {
            return;
        }
        if index != self.selected {
            self.selected = index;
            return;
        }
        self.list_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    fn breadcrumb(&self) -> String {
        let scope = match &self.scope {
            Some(slug) => slug.clone(),
            None => "all projects".into(),
        };
        format!(" {scope} / {}", self.view.name())
    }

    fn list_start(&self, body_height: usize) -> usize {
        self.selected
            .saturating_sub(body_height.saturating_sub(1) / 2)
            .min(self.rows.len().saturating_sub(body_height))
    }

    /// Runs this binary with `args` and keeps its last line as the message.
    fn run(&mut self, args: &[String], stdin: Option<&str>) -> bool {
        let (ok, text) = match args {
            // The CLI refuses safety and profile changes without a person at
            // a terminal; the popup is one, so it writes them here instead.
            [safety, set, target, key, words @ ..] if safety == "safety" && set == "set" => {
                match crate::safety::Target::parse(self.ctx, target)
                    .and_then(|t| crate::safety::apply(self.ctx, &t, key, words))
                {
                    Ok(text) => (true, text),
                    Err(error) => (false, format!("error: {error:#}")),
                }
            }
            [profile, ..] if profile == "profile" => {
                match crate::cli::apply_profile_args(self.ctx, args) {
                    Ok(message) => (true, message),
                    Err(error) => (false, format!("error: {error:#}")),
                }
            }
            _ => run_hp(self.ctx, args, stdin),
        };
        self.message = text;
        self.reload();
        ok
    }

    fn thread_args(row: &ThreadRow, command: &str) -> Vec<String> {
        vec![
            "thread".into(),
            command.into(),
            row.slug.clone(),
            row.thread.id.clone(),
        ]
    }

    fn key(&mut self, key: KeyEvent) {
        let mode = std::mem::replace(&mut self.mode, Mode::List);
        self.mode = match mode {
            Mode::List => {
                self.list_key(key);
                return;
            }
            Mode::Detail {
                title,
                lines,
                files,
                mut selected,
                mut scroll,
            } => match key.code {
                KeyCode::Esc => Mode::List,
                KeyCode::Down | KeyCode::Char('j') => {
                    if !files.is_empty() {
                        selected = (selected + 1).min(files.len() - 1);
                    } else {
                        scroll = (scroll + 1).min(lines.len().saturating_sub(1));
                    }
                    Mode::Detail {
                        title,
                        lines,
                        files,
                        selected,
                        scroll,
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if !files.is_empty() {
                        selected = selected.saturating_sub(1);
                    } else {
                        scroll = scroll.saturating_sub(1);
                    }
                    Mode::Detail {
                        title,
                        lines,
                        files,
                        selected,
                        scroll,
                    }
                }
                KeyCode::PageDown => Mode::Detail {
                    title,
                    lines,
                    files,
                    selected,
                    scroll: scroll + 20,
                },
                KeyCode::PageUp => Mode::Detail {
                    title,
                    lines,
                    files,
                    selected,
                    scroll: scroll.saturating_sub(20),
                },
                KeyCode::Enter if !files.is_empty() => {
                    let mut args = vec![
                        "open-file".to_string(),
                        files[selected].to_string_lossy().into_owned(),
                    ];
                    if !self.workspace.is_empty() {
                        args.extend(["--workspace".into(), self.workspace.clone()]);
                    }
                    if self.run(&args, None) && self.message.contains("new tab") {
                        self.quit = true;
                    }
                    Mode::Detail {
                        title,
                        lines,
                        files,
                        selected,
                        scroll,
                    }
                }
                KeyCode::Char('y') if !files.is_empty() => {
                    self.message = copy(&files[selected].to_string_lossy());
                    Mode::Detail {
                        title,
                        lines,
                        files,
                        selected,
                        scroll,
                    }
                }
                _ => Mode::Detail {
                    title,
                    lines,
                    files,
                    selected,
                    scroll,
                },
            },
            Mode::Confirm {
                question,
                action,
                lines,
            } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.run(&action, None);
                    Mode::List
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Enter => {
                    self.message = "cancelled".into();
                    Mode::List
                }
                _ => Mode::Confirm {
                    question,
                    action,
                    lines,
                },
            },
            Mode::Edit {
                label,
                mut buffer,
                action,
            } => match key.code {
                KeyCode::Esc => {
                    self.message = "cancelled".into();
                    Mode::List
                }
                KeyCode::Enter => {
                    let mut args = action.clone();
                    args.push(buffer.clone());
                    self.run(&args, None);
                    Mode::List
                }
                KeyCode::Backspace => {
                    buffer.pop();
                    Mode::Edit {
                        label,
                        buffer,
                        action,
                    }
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buffer.push(c);
                    Mode::Edit {
                        label,
                        buffer,
                        action,
                    }
                }
                _ => Mode::Edit {
                    label,
                    buffer,
                    action,
                },
            },
            Mode::Pick {
                label,
                options,
                mut selected,
                action,
            } => match key.code {
                KeyCode::Esc => {
                    self.message = "cancelled".into();
                    Mode::List
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                    Mode::Pick {
                        label,
                        options,
                        selected,
                        action,
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(options.len().saturating_sub(1));
                    Mode::Pick {
                        label,
                        options,
                        selected,
                        action,
                    }
                }
                KeyCode::Enter => {
                    let args: Vec<String> = action
                        .iter()
                        .map(|a| {
                            if a == "{}" {
                                options[selected].clone()
                            } else {
                                a.clone()
                            }
                        })
                        .collect();
                    self.run(&args, None);
                    Mode::List
                }
                _ => Mode::Pick {
                    label,
                    options,
                    selected,
                    action,
                },
            },
            Mode::Toggle {
                label,
                mut options,
                mut selected,
                action,
            } => match key.code {
                KeyCode::Esc => {
                    self.message = "cancelled".into();
                    Mode::List
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                    Mode::Toggle {
                        label,
                        options,
                        selected,
                        action,
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(options.len().saturating_sub(1));
                    Mode::Toggle {
                        label,
                        options,
                        selected,
                        action,
                    }
                }
                KeyCode::Char(' ') => {
                    options[selected].1 = !options[selected].1;
                    if selected == 0 && options[0].1 {
                        options.iter_mut().skip(1).for_each(|o| o.1 = false);
                    } else if selected > 0 && options[selected].1 {
                        options[0].1 = false;
                    }
                    Mode::Toggle {
                        label,
                        options,
                        selected,
                        action,
                    }
                }
                KeyCode::Enter => {
                    let mut args = action.clone();
                    if options[0].1 {
                        args.push("--all".into());
                    } else {
                        args.extend(options.iter().skip(1).filter(|o| o.1).map(|o| o.0.clone()));
                        if args.len() == action.len() {
                            self.message =
                                "check at least one profile, or \"every profile\"".into();
                            return self.mode = Mode::Toggle {
                                label,
                                options,
                                selected,
                                action,
                            };
                        }
                    }
                    self.run(&args, None);
                    Mode::List
                }
                _ => Mode::Toggle {
                    label,
                    options,
                    selected,
                    action,
                },
            },
            Mode::Form {
                title,
                mut fields,
                mut selected,
                editing,
            } => match key.code {
                KeyCode::Esc => {
                    self.message = "cancelled".into();
                    Mode::List
                }
                KeyCode::Up | KeyCode::BackTab => {
                    selected = selected.saturating_sub(1);
                    Mode::Form {
                        title,
                        fields,
                        selected,
                        editing,
                    }
                }
                KeyCode::Down | KeyCode::Tab => {
                    selected = (selected + 1).min(fields.len() - 1);
                    Mode::Form {
                        title,
                        fields,
                        selected,
                        editing,
                    }
                }
                KeyCode::Left | KeyCode::Right if !fields[selected].options.is_empty() => {
                    let field = &mut fields[selected];
                    let n = field.options.len();
                    let at = field
                        .options
                        .iter()
                        .position(|o| *o == field.value)
                        .unwrap_or(0);
                    let next = if key.code == KeyCode::Right {
                        (at + 1) % n
                    } else {
                        (at + n - 1) % n
                    };
                    field.value = field.options[next].clone();
                    if field.label == "harness" {
                        // Another harness takes other effort values.
                        fields[3].options = effort_options(&fields[1].value);
                        if !fields[3].options.contains(&fields[3].value) {
                            fields[3].value = String::new();
                        }
                    }
                    Mode::Form {
                        title,
                        fields,
                        selected,
                        editing,
                    }
                }
                KeyCode::Backspace
                    if fields[selected].options.is_empty() && !(editing && selected == 0) =>
                {
                    fields[selected].value.pop();
                    Mode::Form {
                        title,
                        fields,
                        selected,
                        editing,
                    }
                }
                KeyCode::Char(c)
                    if fields[selected].options.is_empty()
                        && !(editing && selected == 0)
                        && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    fields[selected].value.push(c);
                    Mode::Form {
                        title,
                        fields,
                        selected,
                        editing,
                    }
                }
                KeyCode::Enter => {
                    let value = |i: usize| fields[i].value.trim().to_string();
                    let mut args = vec![
                        "profile".to_string(),
                        if editing { "edit" } else { "add" }.into(),
                        value(0),
                        "--agent".into(),
                        value(1),
                        "--model".into(),
                        value(2),
                        "--effort".into(),
                        value(3),
                        "--description".into(),
                        value(5),
                    ];
                    let extra: Vec<String> = fields[4]
                        .value
                        .split_whitespace()
                        .map(str::to_string)
                        .collect();
                    if extra.is_empty() && editing {
                        args.push("--clear-args".into());
                    }
                    for arg in extra {
                        args.push(format!("--arg={arg}"));
                    }
                    if self.run(&args, None) {
                        Mode::List
                    } else {
                        Mode::Form {
                            title,
                            fields,
                            selected,
                            editing,
                        }
                    }
                }
                _ => Mode::Form {
                    title,
                    fields,
                    selected,
                    editing,
                },
            },
            Mode::Projects(mut picker) => match picker.key(key) {
                PickerOutcome::Stay => Mode::Projects(picker),
                PickerOutcome::Close => Mode::List,
                PickerOutcome::Pick(scope) => {
                    self.scope = scope;
                    self.selected = 0;
                    self.reload();
                    Mode::List
                }
            },
            Mode::Start {
                slug,
                phase,
                kind,
                title,
                repo,
                mut buffer,
                options,
                mut selected,
            } => match key.code {
                KeyCode::Esc => {
                    self.message = "cancelled".into();
                    Mode::List
                }
                KeyCode::Up | KeyCode::Char('k') if !options.is_empty() => {
                    selected = selected.saturating_sub(1);
                    Mode::Start {
                        slug,
                        phase,
                        kind,
                        title,
                        repo,
                        buffer,
                        options,
                        selected,
                    }
                }
                KeyCode::Down | KeyCode::Char('j') if !options.is_empty() => {
                    selected = (selected + 1).min(options.len().saturating_sub(1));
                    Mode::Start {
                        slug,
                        phase,
                        kind,
                        title,
                        repo,
                        buffer,
                        options,
                        selected,
                    }
                }
                KeyCode::Backspace if options.is_empty() => {
                    buffer.pop();
                    Mode::Start {
                        slug,
                        phase,
                        kind,
                        title,
                        repo,
                        buffer,
                        options,
                        selected,
                    }
                }
                KeyCode::Char(c)
                    if options.is_empty() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    buffer.push(c);
                    Mode::Start {
                        slug,
                        phase,
                        kind,
                        title,
                        repo,
                        buffer,
                        options,
                        selected,
                    }
                }
                KeyCode::Enter => self.start_advance(Mode::Start {
                    slug,
                    phase,
                    kind,
                    title,
                    repo,
                    buffer,
                    options,
                    selected,
                }),
                _ => Mode::Start {
                    slug,
                    phase,
                    kind,
                    title,
                    repo,
                    buffer,
                    options,
                    selected,
                },
            },
        };
    }

    fn start_advance(&mut self, mode: Mode) -> Mode {
        let Mode::Start {
            slug,
            phase,
            kind,
            title,
            repo,
            buffer,
            options,
            selected,
        } = mode
        else {
            return mode;
        };
        match phase {
            StartPhase::Kind => {
                let kind = options[selected].clone();
                if kind == "tab" || kind == "adopted" {
                    return Mode::Start {
                        slug,
                        phase: StartPhase::Title,
                        kind,
                        title,
                        repo,
                        buffer: String::new(),
                        options: Vec::new(),
                        selected: 0,
                    };
                }
                let repos = Project::load(&self.ctx.root, &slug)
                    .and_then(|project| project.read_project_md())
                    .map(|(settings, _)| settings.repos)
                    .unwrap_or_default();
                if repos.is_empty() {
                    self.message = "a worktree or checkout thread needs a repo".into();
                    return Mode::List;
                }
                if repos.len() == 1 {
                    return Mode::Start {
                        slug,
                        phase: StartPhase::Title,
                        kind,
                        title,
                        repo: repos[0].path.clone(),
                        buffer: String::new(),
                        options: Vec::new(),
                        selected: 0,
                    };
                }
                Mode::Start {
                    slug,
                    phase: StartPhase::Repo,
                    kind,
                    title,
                    repo: String::new(),
                    buffer: String::new(),
                    options: repos.into_iter().map(|repo| repo.path).collect(),
                    selected: 0,
                }
            }
            StartPhase::Repo => Mode::Start {
                slug,
                phase: StartPhase::Title,
                kind,
                title,
                repo: options[selected].clone(),
                buffer: String::new(),
                options: Vec::new(),
                selected: 0,
            },
            StartPhase::Title => {
                let next_title = buffer.trim().to_string();
                if next_title.is_empty() {
                    self.message = "--title may not be empty".into();
                    return Mode::Start {
                        slug,
                        phase,
                        kind,
                        title: String::new(),
                        repo,
                        buffer,
                        options,
                        selected,
                    };
                }
                let next = if kind == "adopted" {
                    StartPhase::Pane
                } else {
                    StartPhase::Task
                };
                Mode::Start {
                    slug,
                    phase: next,
                    kind,
                    title: next_title,
                    repo,
                    buffer: String::new(),
                    options: Vec::new(),
                    selected: 0,
                }
            }
            StartPhase::Task => {
                if buffer.trim().is_empty() {
                    self.message = "the task is empty".into();
                    return Mode::Start {
                        slug,
                        phase,
                        kind,
                        title,
                        repo,
                        buffer,
                        options,
                        selected,
                    };
                }
                let mut args = vec![
                    "thread".into(),
                    "start".into(),
                    slug,
                    "--kind".into(),
                    kind,
                    "--title".into(),
                    title,
                    "--task-file".into(),
                    "-".into(),
                ];
                if !repo.is_empty() {
                    args.extend(["--repo".into(), repo]);
                }
                self.run(&args, Some(&buffer));
                Mode::List
            }
            StartPhase::Pane => {
                let pane = buffer.trim().to_string();
                if pane.is_empty() {
                    self.message = "--pane may not be empty".into();
                    return Mode::Start {
                        slug,
                        phase,
                        kind,
                        title,
                        repo,
                        buffer,
                        options,
                        selected,
                    };
                }
                self.run(
                    &[
                        "thread".into(),
                        "adopt".into(),
                        slug,
                        "--pane".into(),
                        pane,
                        "--title".into(),
                        title,
                    ],
                    None,
                );
                Mode::List
            }
        }
    }

    /// A pick of the profiles `role` may use (in `slug`, or anywhere when
    /// empty), `current` first.
    fn profile_picker(
        &self,
        label: &str,
        slug: &str,
        role: Role,
        current: &str,
        action: Vec<String>,
    ) -> Mode {
        let config = crate::profiles::load(&self.ctx.config_dir).unwrap_or_default();
        let project = Project::load(&self.ctx.root, slug).ok();
        let safety = project
            .as_ref()
            .and_then(|p| p.safety(&self.ctx.config_dir).ok())
            .unwrap_or_default();
        let detected = crate::profiles::detect(self.ctx.env);
        let profiles = if slug.is_empty() {
            config.listed(&detected)
        } else {
            crate::profiles::usable(&config, &safety, role, &detected, current)
        };
        let mut names: Vec<String> = profiles.into_iter().map(|p| p.name).collect();
        if let Some(at) = names.iter().position(|n| n == current) {
            let first = names.remove(at);
            names.insert(0, first);
        }
        if names.is_empty() {
            return Mode::Detail {
                title: label.into(),
                lines: vec![
                    "No profile is allowed here. Allow one in the settings section.".into(),
                ],
                files: Vec::new(),
                selected: 0,
                scroll: 0,
            };
        }
        Mode::Pick {
            label: label.into(),
            options: names,
            selected: 0,
            action,
        }
    }

    /// The allow-list toggle for `role`, in `slug` or for all projects.
    fn allow_toggle(&self, slug: &str, role: Role) -> Mode {
        let config = crate::profiles::load(&self.ctx.config_dir).unwrap_or_default();
        let project = Project::load(&self.ctx.root, slug).ok();
        let safety = project
            .as_ref()
            .and_then(|p| p.safety(&self.ctx.config_dir).ok())
            .unwrap_or_default();
        let allowed = config.allowed(&safety, role);
        let mut names: Vec<String> = config
            .listed(&crate::profiles::detect(self.ctx.env))
            .into_iter()
            .map(|p| p.name)
            .collect();
        for name in allowed.iter().flatten() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        let mut options = vec![(
            "every profile, now and later".to_string(),
            allowed.is_none(),
        )];
        options.extend(names.into_iter().map(|n| {
            let on = allowed.as_ref().is_some_and(|l| l.contains(&n));
            (n, on)
        }));
        let mut action = vec![
            "profile".to_string(),
            "allow".into(),
            if role == Role::Thread {
                "threads"
            } else {
                "coordinator"
            }
            .into(),
        ];
        if !slug.is_empty() {
            action.extend(["--project".into(), slug.to_string()]);
        }
        let scope = if slug.is_empty() {
            "every project without its own list".to_string()
        } else {
            slug.to_string()
        };
        Mode::Toggle {
            label: format!(
                "{} may use, in {scope}",
                if role == Role::Thread {
                    "Threads"
                } else {
                    "Coordinators"
                }
            ),
            options,
            selected: 0,
            action,
        }
    }

    /// The profile form, empty for `n` or filled from profile `name`.
    fn profile_form(&self, name: Option<&str>) -> Mode {
        let config = crate::profiles::load(&self.ctx.config_dir).unwrap_or_default();
        let existing = name.and_then(|n| config.get(n));
        let editing = existing.as_ref().is_some_and(|p| !p.builtin);
        let entry = existing
            .as_ref()
            .map(|p| p.entry.clone())
            .unwrap_or_else(|| crate::profiles::Entry {
                agent: "claude".into(),
                ..Default::default()
            });
        let mut kinds = crate::profiles::detect(self.ctx.env);
        kinds.extend(
            crate::agents::KINDS
                .iter()
                .map(|k| k.to_string())
                .filter(|k| !kinds.contains(k))
                .collect::<Vec<_>>(),
        );
        let fields = vec![
            Field {
                label: "name",
                value: name.unwrap_or_default().to_string(),
                options: Vec::new(),
            },
            Field {
                label: "harness",
                value: entry.agent.clone(),
                options: kinds,
            },
            Field {
                label: "model",
                value: entry.model.clone(),
                options: Vec::new(),
            },
            Field {
                label: "effort",
                value: entry.effort.clone(),
                options: effort_options(&entry.agent),
            },
            Field {
                label: "args",
                value: entry.args.join(" "),
                options: Vec::new(),
            },
            Field {
                label: "description",
                value: entry.description.clone(),
                options: Vec::new(),
            },
        ];
        let title = match (name, editing) {
            (Some(n), true) => format!("Edit profile `{n}`"),
            (Some(n), false) => format!("Replace the built-in `{n}` with your own profile"),
            (None, _) => "New profile".into(),
        };
        Mode::Form {
            title,
            fields,
            selected: if name.is_some() { 1 } else { 0 },
            editing,
        }
    }

    fn list_key(&mut self, key: KeyEvent) {
        self.message.clear();
        if self.leader {
            self.leader = false;
            if let KeyCode::Char(digit) = key.code
                && let Some(view) = View::from_digit(digit)
            {
                self.set_view(view);
                return;
            }
        }
        match key.code {
            KeyCode::Esc => {
                if self.inspect {
                    self.inspect = false;
                } else if self.view.nested() {
                    self.set_view(View::More);
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('g') => self.leader = true,
            KeyCode::Char('h') | KeyCode::Left if self.inspect => self.inspect = false,
            KeyCode::Char('l') | KeyCode::Right => {
                let (width, _) = screen_size();
                if width < 120 {
                    self.inspect = true;
                }
            }
            KeyCode::Char('\\') => {
                let (width, _) = screen_size();
                if width >= 120 {
                    self.hide_projects = !self.hide_projects;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Char(c @ ('P' | '/')) => {
                self.mode = Mode::Projects(Picker::new(
                    picker_rows(&self.ctx.root),
                    self.scope.as_deref(),
                    c == '/',
                ))
            }
            _ => match self.view {
                View::Overview => self.overview_key(key),
                View::NeedsYou | View::Work | View::Reviews => self.thread_key(key),
                View::Places => self.place_key(key),
                View::Tasks => self.task_key(key),
                View::Inbox => self.inbox_key(key),
                View::Automation => self.routine_key(key),
                View::Settings => self.settings_key(key),
                View::More => self.more_key(key),
                View::Memory => {
                    if key.code == KeyCode::Enter
                        && let Some(RowKind::Memory { path }) =
                            self.current().map(|r| r.kind.clone())
                    {
                        let text = std::fs::read_to_string(&path).unwrap_or_default();
                        self.mode = Mode::Detail {
                            title: path.display().to_string(),
                            lines: text.lines().map(str::to_string).collect(),
                            files: Vec::new(),
                            selected: 0,
                            scroll: 0,
                        };
                    }
                }
            },
        }
    }

    fn overview_key(&mut self, key: KeyEvent) {
        if key.code != KeyCode::Enter {
            return;
        }
        let Some(RowKind::Coordinator { slug }) = self.current().map(|row| row.kind.clone()) else {
            return;
        };
        self.run(&["open".into(), slug], None);
    }

    fn more_key(&mut self, key: KeyEvent) {
        if key.code != KeyCode::Enter {
            return;
        }
        match self.current().map(|row| &row.kind) {
            Some(RowKind::MoreMemory) => self.set_view(View::Memory),
            Some(RowKind::MoreSettings) => self.set_view(View::Settings),
            _ => {}
        }
    }

    fn place_key(&mut self, key: KeyEvent) {
        if key.code != KeyCode::Enter {
            return;
        }
        let Some(RowKind::Place {
            unresolved,
            socket,
            machine,
            pane,
            ..
        }) = self.current().map(|row| row.kind.clone())
        else {
            return;
        };
        if unresolved {
            self.message = "unresolved; this command will not run there".into();
            return;
        }
        if pane.is_empty() {
            self.message = "no pane in this place".into();
            return;
        }
        self.jump = Some((socket, machine, pane));
        self.quit = true;
    }

    fn thread_key(&mut self, key: KeyEvent) {
        let slug_for_coordinator =
            self.scope
                .clone()
                .or_else(|| match self.current().map(|r| &r.kind) {
                    Some(RowKind::Thread(r)) => Some(r.slug.clone()),
                    _ => None,
                });
        if key.code == KeyCode::Char('t') && self.view == View::Work {
            let Some(slug) = slug_for_coordinator else {
                self.message = "press P to pick a project first".into();
                return;
            };
            self.mode = Mode::Start {
                slug,
                phase: StartPhase::Kind,
                kind: String::new(),
                title: String::new(),
                repo: String::new(),
                buffer: String::new(),
                options: ["worktree", "tab", "checkout", "adopted"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                selected: 0,
            };
            return;
        }
        if key.code == KeyCode::Char('c') {
            let Some(slug) = slug_for_coordinator else {
                self.message = "select a thread of the project, or press P to pick one".into();
                return;
            };
            let default = Project::load(&self.ctx.root, &slug)
                .and_then(|p| p.read_project_md())
                .map(|(s, _)| s.coordinator_profile)
                .unwrap_or_else(|_| "claude".into());
            let socket = self
                .ctx
                .env
                .var("HERDR_SOCKET_PATH")
                .unwrap_or("")
                .to_string();
            let mut action = vec![
                "open".to_string(),
                slug.clone(),
                "--profile".into(),
                "{}".into(),
            ];
            if !socket.is_empty() {
                action.extend(["--socket".into(), socket]);
            }
            self.mode = self.profile_picker(
                "Start or focus a coordinator with",
                &slug,
                Role::Coordinator,
                &default,
                action,
            );
            return;
        }
        if key.code == KeyCode::Char('S') {
            let Some(slug) = slug_for_coordinator else {
                self.message = "press P to pick a project first".into();
                return;
            };
            let (_, text) = run_hp(
                self.ctx,
                &["sweep".into(), slug.clone(), "--dry-run".into()],
                None,
            );
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            if lines.iter().any(|l| l.starts_with("nothing to clean")) || lines.is_empty() {
                self.message = format!("{slug}: nothing to clean");
            } else {
                self.mode = Mode::Confirm {
                    question: format!(
                        "Remove all {} item(s) listed above from {slug}? y/N",
                        lines.len()
                    ),
                    action: vec!["sweep".into(), slug, "--yes".into()],
                    lines,
                };
            }
            return;
        }
        let Some(RowKind::Thread(row)) = self.current().map(|r| r.kind.clone()) else {
            return;
        };
        let t = &row.thread;
        match key.code {
            KeyCode::Enter => {
                if t.status == thread::Status::Resolved
                    || t.pane_id.is_empty()
                    || t.state_line.contains("pane closed")
                {
                    self.mode = detail(&self.ctx.root, &row);
                } else {
                    self.jump = Some((row.socket.clone(), t.machine.clone(), t.pane_id.clone()));
                    self.quit = true;
                }
            }
            KeyCode::Char('i') => self.mode = detail(&self.ctx.root, &row),
            KeyCode::Char(c @ '1'..='9') => {
                let n = c.to_digit(10).unwrap_or(0) as usize;
                if n > row.next.len() {
                    self.message = if row.next.is_empty() {
                        format!("{} has no Next list", t.id)
                    } else {
                        format!("{} has {} Next line(s)", t.id, row.next.len())
                    };
                } else {
                    let mut args = Self::thread_args(&row, "next");
                    args.extend(["--line".into(), n.to_string()]);
                    self.run(&args, None);
                }
            }
            KeyCode::Char('s') => {
                self.run(&Self::thread_args(&row, "stop"), None);
            }
            KeyCode::Char('a') => {
                self.run(&Self::thread_args(&row, "ack"), None);
            }
            KeyCode::Char('r') => {
                let mut action = Self::thread_args(&row, "restart");
                action.extend(["--profile".into(), "{}".into()]);
                let current = if t.profile.is_empty() {
                    &t.agent
                } else {
                    &t.profile
                };
                self.mode = self.profile_picker(
                    &format!("Restart {} with", t.id),
                    &row.slug,
                    Role::Thread,
                    current,
                    action,
                );
            }
            KeyCode::Char('x') => {
                self.mode = Mode::Confirm {
                    question: format!(
                        "Resolve {} \"{}\" and clean up its worktree, panes and merged branch? y/N",
                        t.id, t.title
                    ),
                    action: Self::thread_args(&row, "resolve"),
                    lines: Vec::new(),
                };
            }
            KeyCode::Char('o') => {
                if t.pr.is_empty() {
                    self.message = format!("{} has no pull request", t.id);
                } else {
                    self.run(&["open-url".into(), t.pr.clone()], None);
                }
            }
            _ => {}
        }
    }

    fn coordinator_says(&mut self, slug: &str, text: String) {
        self.run(
            &[
                "coordinator".into(),
                "prompt".into(),
                slug.to_string(),
                "--text-file".into(),
                "-".into(),
            ],
            Some(&text),
        );
    }

    fn task_key(&mut self, key: KeyEvent) {
        let Some(RowKind::Task(task)) = self.current().map(|r| r.kind.clone()) else {
            return;
        };
        let sentence = |verb: &str| {
            format!(
                "(from the projects popup) {verb} the task \"{}\" in TASKS.md.",
                task.title
            )
        };
        match key.code {
            KeyCode::Enter => match &task.thread {
                Some(id) => {
                    if let Some(row) = thread_rows(&self.ctx.root, Some(&task.slug))
                        .into_iter()
                        .find(|r| &r.thread.id == id)
                    {
                        if row.thread.pane_id.is_empty()
                            || row.thread.status == thread::Status::Resolved
                        {
                            self.mode = detail(&self.ctx.root, &row);
                        } else {
                            self.jump = Some((
                                row.socket.clone(),
                                row.thread.machine.clone(),
                                row.thread.pane_id.clone(),
                            ));
                            self.quit = true;
                        }
                    }
                }
                None if !task.description.trim().is_empty() => self.mode = task_detail(&task),
                None => self.message = "this task has no thread yet; d delegates it".into(),
            },
            KeyCode::Char('i') => self.mode = task_detail(&task),
            KeyCode::Char('d') => self.coordinator_says(&task.slug, sentence("Please delegate")),
            KeyCode::Char('m') => {
                self.coordinator_says(&task.slug, sentence("Please mark as done"))
            }
            KeyCode::Char('D') => self.coordinator_says(&task.slug, sentence("Please drop")),
            _ => {}
        }
    }

    fn inbox_key(&mut self, key: KeyEvent) {
        let Some(RowKind::Inbox { slug, id, body }) = self.current().map(|r| r.kind.clone()) else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                self.mode = Mode::Detail {
                    title: id,
                    lines: body.lines().map(str::to_string).collect(),
                    files: Vec::new(),
                    selected: 0,
                    scroll: 0,
                }
            }
            KeyCode::Char('a') => {
                self.run(&["inbox".into(), "done".into(), slug, id], None);
            }
            _ => {}
        }
    }

    fn routine_key(&mut self, key: KeyEvent) {
        let Some(RowKind::Routine { slug, name, prompt }) = self.current().map(|r| r.kind.clone())
        else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                self.run(&["routine".into(), "toggle".into(), slug, name], None);
            }
            KeyCode::Char('i') => {
                self.mode = Mode::Detail {
                    title: name,
                    lines: prompt.lines().map(str::to_string).collect(),
                    files: Vec::new(),
                    selected: 0,
                    scroll: 0,
                }
            }
            _ => {}
        }
    }

    /// `Y`: flips yolo for the popup's scope (all projects when unscoped),
    /// after a y/N question when it turns on.
    fn yolo_key(&mut self) {
        let target = self.scope.clone().unwrap_or_else(|| "--global".into());
        let label = self.scope.clone().unwrap_or_else(|| "all projects".into());
        let on = safety_rows(self.ctx, self.scope.as_deref()).iter().any(|r| matches!(&r.kind, RowKind::Safety { key, value, .. } if key == "yolo" && value == "on"));
        let action: Vec<String> = [
            "safety",
            "set",
            &target,
            "yolo",
            if on { "off" } else { "on" },
        ]
        .map(String::from)
        .to_vec();
        if on {
            self.run(&action, None);
        } else {
            self.mode = Mode::Confirm {
                question: format!(
                    "Yolo for {label}? Threads start without asking; agents run with no permission prompts. y/N"
                ),
                action,
                lines: Vec::new(),
            };
        }
    }

    fn settings_key(&mut self, key: KeyEvent) {
        if self.scope.is_none() && key.code == KeyCode::Char('n') {
            self.mode = self.profile_form(None);
            return;
        }
        if key.code == KeyCode::Char('Y') {
            self.yolo_key();
            return;
        }
        match self.current().map(|r| r.kind.clone()) {
            Some(RowKind::Profile { name, builtin }) => match key.code {
                KeyCode::Enter => self.mode = self.profile_form(Some(&name)),
                KeyCode::Char('d') if builtin => {
                    self.message = format!(
                        "`{name}` is built in: it shows while its CLI is installed and signed in"
                    )
                }
                KeyCode::Char('d') => {
                    self.mode = Mode::Confirm {
                        question: format!(
                            "Delete profile `{name}`? Threads that use it will not launch again. y/N"
                        ),
                        action: vec!["profile".into(), "remove".into(), name],
                        lines: Vec::new(),
                    };
                }
                _ => {}
            },
            Some(RowKind::Setting {
                slug,
                key: name,
                value,
            }) if slug.is_empty() => {
                if key.code == KeyCode::Enter {
                    let role = if name.starts_with("thread") {
                        Role::Thread
                    } else {
                        Role::Coordinator
                    };
                    let word = if role == Role::Thread {
                        "threads"
                    } else {
                        "coordinator"
                    };
                    self.mode = if name.ends_with("_profiles") {
                        self.allow_toggle("", role)
                    } else {
                        self.profile_picker(
                            &format!("{name} for new projects"),
                            "",
                            role,
                            &value,
                            vec!["profile".into(), "default".into(), word.into(), "{}".into()],
                        )
                    };
                }
            }
            Some(RowKind::Safety {
                slug,
                key: name,
                value,
            }) => {
                if key.code == KeyCode::Enter {
                    let target = slug.clone().unwrap_or_else(|| "--global".into());
                    let action = vec!["safety".to_string(), "set".into(), target, name.clone()];
                    let pick = |options: &[&str]| {
                        let mut options: Vec<String> =
                            options.iter().map(|o| o.to_string()).collect();
                        if slug.is_some() {
                            options.push("default".into());
                        }
                        let mut action = action.clone();
                        action.push("{}".into());
                        Mode::Pick {
                            label: format!("{name} (default: use the all-projects value)"),
                            options,
                            selected: 0,
                            action,
                        }
                    };
                    self.mode = match name.as_str() {
                        "yolo" | "routine_commands" if value == "on" => pick(&["off", "on"]),
                        "yolo" | "routine_commands" => pick(&["on", "off"]),
                        "start_threads" if value == "auto" => pick(&["propose", "auto"]),
                        "start_threads" => pick(&["auto", "propose"]),
                        "trust_screens" if value == "coordinator" => pick(&["user", "coordinator"]),
                        "trust_screens" => pick(&["coordinator", "user"]),
                        _ => Mode::Edit {
                            label: format!("{name} (space-separated; empty for none)"),
                            buffer: if value == "(none)" {
                                String::new()
                            } else {
                                value
                            },
                            action,
                        },
                    };
                } else if let Some(slug) = slug {
                    self.project_key(key, &slug);
                }
            }
            Some(RowKind::Project { slug }) => {
                if key.code == KeyCode::Enter {
                    self.scope = Some(slug);
                    self.selected = 0;
                    self.reload();
                }
            }
            Some(RowKind::Setting {
                slug,
                key: name,
                value,
            }) => match key.code {
                KeyCode::Enter => {
                    let action = vec!["set".to_string(), slug.clone(), name.clone()];
                    self.mode = match name.as_str() {
                        "coordinator_profile" | "thread_profile" => {
                            let mut action = action;
                            action.push("{}".into());
                            let role = if name == "thread_profile" {
                                Role::Thread
                            } else {
                                Role::Coordinator
                            };
                            self.profile_picker(&name, &slug, role, &value, action)
                        }
                        "thread_profiles" => self.allow_toggle(&slug, Role::Thread),
                        "coordinator_profiles" => self.allow_toggle(&slug, Role::Coordinator),
                        "nudge" | "mute" => {
                            let mut action = action;
                            action.push("{}".into());
                            Mode::Pick {
                                label: name.clone(),
                                options: vec![(value != "true").to_string(), value.clone()],
                                selected: 0,
                                action,
                            }
                        }
                        "repos.remove" => {
                            let options: Vec<String> = value
                                .split(", ")
                                .filter(|s| *s != "(none)")
                                .map(str::to_string)
                                .collect();
                            if options.is_empty() {
                                self.message = "no repos to remove".into();
                                Mode::List
                            } else {
                                let mut action = action;
                                action.push("{}".into());
                                Mode::Pick {
                                    label: "remove repo".into(),
                                    options,
                                    selected: 0,
                                    action,
                                }
                            }
                        }
                        "repos.add" => Mode::Edit {
                            label: "add repo (PATH or PATH@MACHINE)".into(),
                            buffer: String::new(),
                            action,
                        },
                        _ => Mode::Edit {
                            label: name.clone(),
                            buffer: value,
                            action,
                        },
                    };
                }
                _ => self.project_key(key, &slug),
            },
            _ => {
                if let Some(slug) = self.scope.clone() {
                    self.project_key(key, &slug);
                }
            }
        }
    }

    fn project_key(&mut self, key: KeyEvent, slug: &str) {
        let status = Project::load(&self.ctx.root, slug)
            .map(|p| p.status())
            .unwrap_or_default();
        match key.code {
            KeyCode::Char('p') => {
                let verb = if status == Status::Paused {
                    "resume"
                } else {
                    "pause"
                };
                self.run(&[verb.into(), slug.to_string()], None);
            }
            KeyCode::Char('A') => {
                self.mode = Mode::Confirm {
                    question: format!(
                        "Archive {slug}? Its workspace closes and it is hidden; the folder stays. y/N"
                    ),
                    action: vec!["archive".into(), slug.to_string()],
                    lines: Vec::new(),
                };
            }
            KeyCode::Char('X') => {
                self.mode = Mode::Confirm {
                    question: format!("Delete {slug}? Its folder moves to the trash. y/N"),
                    action: vec!["delete".into(), slug.to_string(), "--force".into()],
                    lines: Vec::new(),
                };
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    fn paint_row(
        &self,
        out: &mut impl std::io::Write,
        row: &Row,
        selected: bool,
        width: usize,
    ) -> std::io::Result<()> {
        let text = paint_list_row(row, selected, width);
        if row.header {
            queue!(
                out,
                SetAttribute(Attribute::Bold),
                Print(text),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            if selected {
                queue!(out, SetAttribute(Attribute::Reverse))?;
            }
            if let Some(color) = row.color {
                queue!(out, SetForegroundColor(color))?;
            }
            queue!(out, Print(text), ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    fn draw_columns(
        &self,
        out: &mut impl std::io::Write,
        width: usize,
        body_top: usize,
        body_height: usize,
    ) -> std::io::Result<()> {
        let (project_w, body_w, inspector_w) = column_widths(width, self.hide_projects);
        if project_w > 0 {
            for (i, project) in picker_rows(&self.ctx.root)
                .iter()
                .take(body_height)
                .enumerate()
            {
                let name = match &project.slug {
                    Some(slug) if slug != &project.name => format!("{} ({slug})", project.name),
                    _ => project.name.clone(),
                };
                queue!(out, cursor::MoveTo(0, (body_top + i) as u16))?;
                let text = fit(&format!(" {name}"), project_w);
                if project.slug == self.scope {
                    queue!(
                        out,
                        SetAttribute(Attribute::Reverse),
                        Print(text),
                        SetAttribute(Attribute::Reset)
                    )?;
                } else {
                    queue!(out, Print(text))?;
                }
            }
        }
        if width >= 160 && self.view == View::Places {
            let left = body_w / 2;
            let right = body_w.saturating_sub(left);
            let headers: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.header)
                .map(|(index, _)| index)
                .collect();
            let group = self
                .rows
                .iter()
                .take(self.selected + 1)
                .rposition(|row| row.header);
            let places = group
                .map(|start| {
                    let end = self
                        .rows
                        .iter()
                        .enumerate()
                        .skip(start + 1)
                        .find(|(_, row)| row.header)
                        .map(|(index, _)| index)
                        .unwrap_or(self.rows.len());
                    (start + 1..end).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for (i, index) in headers.iter().take(body_height).enumerate() {
                queue!(out, cursor::MoveTo(project_w as u16, (body_top + i) as u16))?;
                self.paint_row(out, &self.rows[*index], Some(*index) == group, left)?;
            }
            for (i, index) in places.iter().take(body_height).enumerate() {
                queue!(
                    out,
                    cursor::MoveTo((project_w + left) as u16, (body_top + i) as u16)
                )?;
                self.paint_row(out, &self.rows[*index], *index == self.selected, right)?;
            }
        } else {
            let start = self.list_start(body_height);
            for (i, row) in self.rows.iter().enumerate().skip(start).take(body_height) {
                queue!(
                    out,
                    cursor::MoveTo(project_w as u16, (body_top + i - start) as u16)
                )?;
                self.paint_row(out, row, i == self.selected, body_w)?;
            }
        }
        if inspector_w > 0 {
            let lines = self.current().map(inspector_lines).unwrap_or_default();
            let x = (project_w + body_w) as u16;
            for (i, line) in lines.iter().take(body_height).enumerate() {
                queue!(
                    out,
                    cursor::MoveTo(x, (body_top + i) as u16),
                    SetAttribute(Attribute::Dim),
                    Print(fit(line, inspector_w)),
                    SetAttribute(Attribute::Reset)
                )?;
            }
        }
        Ok(())
    }

    fn draw(&self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        let (width, height) = screen_size();
        queue!(
            out,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0)
        )?;
        let left = self.breadcrumb();
        let right = summary(&self.ctx.root);
        let pad = width.saturating_sub(left.chars().count() + right.chars().count() + 1);
        let left_text: String = left.chars().take(width).collect();
        queue!(
            out,
            SetAttribute(Attribute::Bold),
            Print(left_text),
            SetAttribute(Attribute::Reset)
        )?;
        if pad > 0 {
            queue!(
                out,
                Print(" ".repeat(pad)),
                SetAttribute(Attribute::Dim),
                Print(&right),
                SetAttribute(Attribute::Reset)
            )?;
        }
        queue!(out, cursor::MoveTo(0, 1), Print("─".repeat(width)))?;

        let (body_top, body_height) = body_window(height);
        match &self.mode {
            Mode::Detail {
                title,
                lines,
                files,
                selected,
                scroll,
            } => {
                queue!(
                    out,
                    cursor::MoveTo(0, body_top as u16),
                    SetAttribute(Attribute::Bold),
                    Print(fit(&format!(" {title}"), width)),
                    SetAttribute(Attribute::Reset)
                )?;
                let file_start = lines.len();
                let all: Vec<String> = lines
                    .iter()
                    .cloned()
                    .chain(files.iter().map(|f| format!("  {}", f.display())))
                    .collect();
                let start = if files.is_empty() {
                    *scroll
                } else {
                    (file_start + selected).saturating_sub(body_height.saturating_sub(2))
                };
                for (i, line) in all
                    .iter()
                    .skip(start)
                    .take(body_height.saturating_sub(1))
                    .enumerate()
                {
                    let index = start + i;
                    queue!(out, cursor::MoveTo(0, (body_top + 1 + i) as u16))?;
                    if !files.is_empty() && index == file_start + selected {
                        queue!(
                            out,
                            SetAttribute(Attribute::Reverse),
                            Print(fit(line, width)),
                            SetAttribute(Attribute::Reset)
                        )?;
                    } else {
                        queue!(out, Print(fit(line, width)))?;
                    }
                }
            }
            Mode::Confirm { lines, .. } if !lines.is_empty() => {
                queue!(
                    out,
                    cursor::MoveTo(0, body_top as u16),
                    SetAttribute(Attribute::Bold),
                    Print(fit(" This would remove:", width)),
                    SetAttribute(Attribute::Reset)
                )?;
                for (i, line) in lines.iter().take(body_height.saturating_sub(2)).enumerate() {
                    queue!(
                        out,
                        cursor::MoveTo(0, (body_top + 1 + i) as u16),
                        Print(fit(&format!("  {line}"), width))
                    )?;
                }
                if lines.len() > body_height.saturating_sub(2) {
                    queue!(
                        out,
                        cursor::MoveTo(0, (body_top + body_height - 1) as u16),
                        Print(fit(
                            &format!(
                                "  … and {} more (run `sweep --dry-run` to see all)",
                                lines.len() - body_height + 2
                            ),
                            width
                        ))
                    )?;
                }
            }
            Mode::Pick {
                label,
                options,
                selected,
                ..
            } => {
                queue!(
                    out,
                    cursor::MoveTo(0, body_top as u16),
                    SetAttribute(Attribute::Bold),
                    Print(fit(&format!(" {label}"), width)),
                    SetAttribute(Attribute::Reset)
                )?;
                let start = selected.saturating_sub(body_height.saturating_sub(2));
                for (i, option) in options
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(body_height.saturating_sub(1))
                {
                    queue!(out, cursor::MoveTo(0, (body_top + 1 + i - start) as u16))?;
                    let text = fit(&format!("  {option}"), width);
                    if i == *selected {
                        queue!(
                            out,
                            SetAttribute(Attribute::Reverse),
                            Print(text),
                            SetAttribute(Attribute::Reset)
                        )?;
                    } else {
                        queue!(out, Print(text))?;
                    }
                }
            }
            Mode::Toggle {
                label,
                options,
                selected,
                ..
            } => {
                queue!(
                    out,
                    cursor::MoveTo(0, body_top as u16),
                    SetAttribute(Attribute::Bold),
                    Print(fit(&format!(" {label}"), width)),
                    SetAttribute(Attribute::Reset)
                )?;
                let start = selected.saturating_sub(body_height.saturating_sub(2));
                for (i, (option, on)) in options
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(body_height.saturating_sub(1))
                {
                    queue!(out, cursor::MoveTo(0, (body_top + 1 + i - start) as u16))?;
                    let text = fit(
                        &format!("  [{}] {option}", if *on { "x" } else { " " }),
                        width,
                    );
                    if i == *selected {
                        queue!(
                            out,
                            SetAttribute(Attribute::Reverse),
                            Print(text),
                            SetAttribute(Attribute::Reset)
                        )?;
                    } else {
                        queue!(out, Print(text))?;
                    }
                }
            }
            Mode::Form {
                title,
                fields,
                selected,
                editing,
            } => {
                queue!(
                    out,
                    cursor::MoveTo(0, body_top as u16),
                    SetAttribute(Attribute::Bold),
                    Print(fit(&format!(" {title}"), width)),
                    SetAttribute(Attribute::Reset)
                )?;
                for (i, field) in fields.iter().enumerate() {
                    queue!(out, cursor::MoveTo(0, (body_top + 2 + i) as u16))?;
                    let value = if field.options.is_empty() {
                        let cursor = if i == *selected && !(*editing && i == 0) {
                            "▏"
                        } else {
                            ""
                        };
                        format!("{}{cursor}", field.value)
                    } else {
                        format!(
                            "‹ {} ›",
                            if field.value.is_empty() {
                                "(default)"
                            } else {
                                &field.value
                            }
                        )
                    };
                    let text = fit(&format!("  {:<12} {value}", field.label), width);
                    if i == *selected {
                        queue!(
                            out,
                            SetAttribute(Attribute::Reverse),
                            Print(text),
                            SetAttribute(Attribute::Reset)
                        )?;
                    } else {
                        queue!(out, Print(text))?;
                    }
                }
                let help = [
                    "",
                    "  args: extra CLI arguments, separated by spaces (e.g. --config ~/.omp/agent/luna.yml).",
                    "  Effort maps to each harness's own flag; harnesses without one show only (default).",
                    "  Profiles live in ~/.config/herdr-projects/config.toml, which agents cannot change.",
                ];
                for (i, line) in help.iter().enumerate() {
                    queue!(
                        out,
                        cursor::MoveTo(0, (body_top + 2 + fields.len() + i) as u16),
                        SetAttribute(Attribute::Dim),
                        Print(fit(line, width)),
                        SetAttribute(Attribute::Reset)
                    )?;
                }
            }
            Mode::Projects(picker) => {
                let title = match &picker.filter {
                    Some(filter) => format!(" Switch to project  / {filter}▏"),
                    None => " Switch to project".to_string(),
                };
                queue!(
                    out,
                    cursor::MoveTo(0, body_top as u16),
                    SetAttribute(Attribute::Bold),
                    Print(fit(&title, width)),
                    SetAttribute(Attribute::Reset)
                )?;
                let visible = picker.visible();
                if visible.is_empty() {
                    queue!(
                        out,
                        cursor::MoveTo(0, (body_top + 1) as u16),
                        SetAttribute(Attribute::Dim),
                        Print(fit("  no projects match", width)),
                        SetAttribute(Attribute::Reset)
                    )?;
                }
                let start = picker
                    .selected
                    .saturating_sub(body_height.saturating_sub(2));
                for (i, row) in visible
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(body_height.saturating_sub(1))
                {
                    queue!(out, cursor::MoveTo(0, (body_top + 1 + i - start) as u16))?;
                    let current = if row.slug == self.scope { "•" } else { " " };
                    let label = match &row.slug {
                        Some(slug) if *slug != row.name => format!("{} ({slug})", row.name),
                        _ => row.name.clone(),
                    };
                    let text = fit(&format!(" {current} {label} · {}", row.status), width);
                    if i == picker.selected {
                        queue!(
                            out,
                            SetAttribute(Attribute::Reverse),
                            Print(text),
                            SetAttribute(Attribute::Reset)
                        )?;
                    } else {
                        queue!(out, Print(text))?;
                    }
                }
            }
            Mode::Start {
                phase,
                buffer,
                options,
                selected,
                ..
            } => {
                queue!(
                    out,
                    cursor::MoveTo(0, body_top as u16),
                    SetAttribute(Attribute::Bold),
                    Print(fit(&format!(" {}", phase.label()), width)),
                    SetAttribute(Attribute::Reset)
                )?;
                if options.is_empty() {
                    queue!(
                        out,
                        cursor::MoveTo(0, (body_top + 1) as u16),
                        SetAttribute(Attribute::Reverse),
                        Print(fit(&format!("  {buffer}▏"), width)),
                        SetAttribute(Attribute::Reset)
                    )?;
                } else {
                    let start = selected.saturating_sub(body_height.saturating_sub(2));
                    for (i, option) in options
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(body_height.saturating_sub(1))
                    {
                        queue!(out, cursor::MoveTo(0, (body_top + 1 + i - start) as u16))?;
                        let text = fit(&format!("  {option}"), width);
                        if i == *selected {
                            queue!(
                                out,
                                SetAttribute(Attribute::Reverse),
                                Print(text),
                                SetAttribute(Attribute::Reset)
                            )?;
                        } else {
                            queue!(out, Print(text))?;
                        }
                    }
                }
            }
            _ => {
                let (_, _, inspector_w) = column_widths(width, self.hide_projects);
                if self.inspect && inspector_w == 0 {
                    let lines = self.current().map(inspector_lines).unwrap_or_default();
                    for (i, line) in lines.iter().take(body_height).enumerate() {
                        queue!(
                            out,
                            cursor::MoveTo(0, (body_top + i) as u16),
                            SetAttribute(Attribute::Dim),
                            Print(fit(line, width)),
                            SetAttribute(Attribute::Reset)
                        )?;
                    }
                } else {
                    self.draw_columns(out, width, body_top, body_height)?;
                }
            }
        }

        // Footer: the message or prompt, then the keys.
        let footer = height.saturating_sub(2) as u16;
        queue!(
            out,
            cursor::MoveTo(0, footer),
            Print("─".repeat(width)),
            cursor::MoveTo(0, footer + 1)
        )?;
        let hint = match &self.mode {
            Mode::List if self.inspect && width < 120 => "esc back".into(),
            Mode::List => format!(
                "{}  g 1-9 view  \\ projects  P project  / find  {}",
                self.view.keys(),
                if self.view.nested() {
                    "esc back"
                } else {
                    "esc close"
                }
            ),
            Mode::Start { options, .. } if options.is_empty() => "type  ↵ next  esc cancel".into(),
            Mode::Start { .. } => "↑↓ choose  ↵ next  esc cancel".into(),
            Mode::Projects(Picker {
                filter: Some(_), ..
            }) => "type to filter  ↑↓ choose  ↵ switch  esc clear/close".into(),
            Mode::Projects(_) => "↑↓ choose  ↵ switch  / filter  esc close".into(),
            Mode::Detail { files, .. } if !files.is_empty() => {
                "↑↓ file  ↵ open  y copy path  esc back".into()
            }
            Mode::Detail { .. } => "↑↓ scroll  esc back".into(),
            Mode::Confirm { question, .. } => question.clone(),
            Mode::Edit { label, buffer, .. } => format!("{label}: {buffer}▏  ↵ save  esc cancel"),
            Mode::Pick { .. } => "↑↓ choose  ↵ ok  esc cancel".into(),
            Mode::Toggle { .. } => "↑↓ choose  space check  ↵ save  esc cancel".into(),
            Mode::Form { .. } => "↑↓/tab field  type to edit  ←→ choose  ↵ save  esc cancel".into(),
        };
        let line = if self.message.is_empty()
            || matches!(self.mode, Mode::Confirm { .. } | Mode::Edit { .. })
        {
            hint
        } else {
            format!("{}  │  {hint}", self.message)
        };
        queue!(
            out,
            SetAttribute(Attribute::Dim),
            Print(fit(&format!(" {line}"), width)),
            SetAttribute(Attribute::Reset)
        )?;
        out.flush()
    }
}

/// A thread's detail: report, Next list, then its files (selectable).
fn detail(root: &Path, row: &ThreadRow) -> Mode {
    let t = &row.thread;
    let Ok(project) = Project::load(root, &row.slug) else {
        return Mode::List;
    };
    let mut lines = vec![format!(
        "{} · {} · {}",
        t.id,
        thread_word(t, row.group),
        t.title
    )];
    if !row.pr_facts.is_empty() {
        lines.push(row.pr_facts.clone());
    }
    lines.push(String::new());
    let report = std::fs::read_to_string(thread::home_report_path(&project, &t.id))
        .unwrap_or_else(|_| "(no report yet)".into());
    lines.extend(report.lines().map(str::to_string));
    if !row.next.is_empty() {
        lines.push(String::new());
        lines.push("Next (press the number in the list):".into());
        for (i, n) in row.next.iter().enumerate() {
            lines.push(format!("  {}. {n}", i + 1));
        }
    }
    let mut files = Vec::new();
    for dir in [
        project.dir().join("library").join(&t.id),
        project.dir().join("uploads"),
    ] {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|e| {
                e.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file())
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        files.extend(found);
    }
    if !files.is_empty() {
        lines.push(String::new());
        lines.push("Files (↵ opens, y copies the path):".into());
    }
    Mode::Detail {
        title: format!("{} · {}", row.slug, t.id),
        lines,
        files,
        selected: 0,
        scroll: 0,
    }
}

fn row_identity(row: &Row) -> Option<String> {
    match &row.kind {
        RowKind::Thread(thread) => Some(format!("{}:{}", thread.slug, thread.thread.id)),
        RowKind::Coordinator { slug } => Some(format!("coordinator:{slug}")),
        RowKind::Place {
            slug,
            cwd,
            repository_id,
            ..
        } => Some(format!(
            "place:{slug}:{}:{cwd}",
            repository_id.as_deref().unwrap_or("")
        )),
        _ => None,
    }
}

fn column_widths(width: usize, hide_projects: bool) -> (usize, usize, usize) {
    if width < 120 {
        return (0, width, 0);
    }
    let inspector = 32.min(width / 4);
    let project = if hide_projects { 0 } else { 22.min(width / 5) };
    let body = width.saturating_sub(project + inspector);
    (project, body, inspector)
}

fn inspector_lines(row: &Row) -> Vec<String> {
    match &row.kind {
        RowKind::Thread(thread) => {
            let t = &thread.thread;
            let mut lines = vec![
                t.id.clone(),
                thread_word(t, thread.group).to_string(),
                format!("{:?}", t.kind).to_lowercase(),
                format!("{:?}", t.status).to_lowercase(),
            ];
            if !t.pr.is_empty() {
                lines.push(t.pr.clone());
            }
            if !t.profile.is_empty() {
                lines.push(t.profile.clone());
            } else if !t.agent.is_empty() {
                lines.push(t.agent.clone());
            }
            lines
        }
        RowKind::Coordinator { slug } => {
            vec![format!("coordinator {slug}"), row.text.trim().into()]
        }
        RowKind::Place {
            cwd, unresolved, ..
        } => {
            let mut lines = vec![cwd.clone()];
            if *unresolved {
                lines.push("unresolved".into());
            }
            lines
        }
        _ => vec![row.text.trim().to_string()],
    }
}

fn screen_size() -> (usize, usize) {
    let (width, height) = terminal::size().unwrap_or((100, 30));
    (width as usize, height as usize)
}

fn body_window(height: usize) -> (usize, usize) {
    (2, height.saturating_sub(4))
}

fn paint_list_row(row: &Row, selected: bool, width: usize) -> String {
    let marker = if selected && !row.header { "▌" } else { " " };
    fit(&format!("{marker}{}", row.text), width)
}

fn fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        format!("{text}{}", " ".repeat(width - count))
    } else {
        let cut: String = text.chars().take(width.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Copies text to the clipboard with the platform's tool.
fn copy(text: &str) -> String {
    use std::process::{Command, Stdio};
    let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };
    for (tool, args) in tools {
        if let Ok(mut child) = Command::new(tool)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                return format!("copied {text}");
            }
        }
    }
    format!("no clipboard tool found; the path is {text}")
}

/// Runs this binary with the same root; (success, last line of output).
pub fn run_hp(ctx: &Ctx, args: &[String], stdin: Option<&str>) -> (bool, String) {
    let Ok(binary) = std::env::current_exe() else {
        return (false, "could not find this binary".into());
    };
    // Sweep and resolve may remove many worktrees; allow them time.
    let mut cmd = crate::runner::Cmd::new(binary.to_string_lossy(), Duration::from_secs(600))
        .arg("--root")
        .arg(ctx.root.to_string_lossy())
        .args(args.iter().cloned());
    if let Some(text) = stdin {
        cmd = cmd.stdin(text);
    }
    match ctx.runner.run(&cmd) {
        Ok(out) => {
            let text = if out.success() {
                out.stdout.clone()
            } else {
                format!("{}\n{}", out.stdout, out.stderr)
            };
            let last = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("done")
                .trim()
                .trim_start_matches("herdr-projects: ")
                .to_string();
            (
                out.success(),
                if out.success() {
                    last
                } else {
                    format!("error: {last}")
                },
            )
        }
        Err(error) => (false, format!("error: {error:#}")),
    }
}

/// Focuses a pane after the popup closed. Herdr's docs do not say focus is
/// refused while a popup is up; if it is, a detached child retries shortly
/// after this process (and with it the popup) has exited.
fn focus(ctx: &Ctx, socket: &str, machine: &str, pane: &str) {
    let herdr =
        crate::herdr::Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner).on_machine(machine);
    if herdr.agent_focus(pane).is_ok() {
        return;
    }
    use std::os::unix::process::CommandExt;
    let mut args = Vec::new();
    if !machine.is_empty() {
        args.extend(["--machine".to_string(), machine.to_string()]);
    }
    args.extend(["agent".to_string(), "focus".to_string(), pane.to_string()]);
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args(["-c", "sleep 0.2; exec \"$@\"", "sh", &ctx.env.herdr_bin()])
        .args(&args)
        .env("HERDR_SOCKET_PATH", socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        command.pre_exec(|| {
            unsafe extern "C" {
                fn setsid() -> i32;
            }
            setsid();
            Ok(())
        });
    }
    let _ = command.spawn();
}

enum TermEvent {
    Tick,
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize,
}

struct Events {
    rx: mpsc::Receiver<TermEvent>,
}

impl Events {
    fn start(tick: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        spawn(move || {
            let mut last = Instant::now();
            loop {
                let timeout = tick.saturating_sub(last.elapsed());
                let sent = match event::poll(timeout) {
                    Ok(true) => match event::read() {
                        Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                            tx.send(TermEvent::Key(key))
                        }
                        Ok(Event::Mouse(mouse))
                            if !matches!(
                                mouse.kind,
                                MouseEventKind::Moved | MouseEventKind::Drag(_)
                            ) =>
                        {
                            tx.send(TermEvent::Mouse(mouse))
                        }
                        Ok(Event::Resize(..)) => tx.send(TermEvent::Resize),
                        Ok(_) => Ok(()),
                        Err(_) => break,
                    },
                    Ok(false) => Ok(()),
                    Err(_) => break,
                };
                if sent.is_err() {
                    break;
                }
                if last.elapsed() >= tick {
                    if tx.send(TermEvent::Tick).is_err() {
                        break;
                    }
                    last = Instant::now();
                }
            }
        });
        Self { rx }
    }

    fn next(&self) -> Result<TermEvent> {
        self.rx
            .recv()
            .map_err(|_| anyhow::anyhow!("the popup stopped receiving terminal events"))
    }
}

/// The popup's loop, on the terminal Herdr gives the popup (or any terminal,
/// through `popup [slug]`).
pub fn run(ctx: &Ctx, scope: Option<String>, workspace: String) -> Result<()> {
    let mut popup = Popup::new(ctx, scope, workspace);
    let mut out = std::io::stdout();
    terminal::enable_raw_mode()?;
    execute!(
        out,
        terminal::EnterAlternateScreen,
        cursor::Hide,
        EnableMouseCapture
    )?;
    // Clicks and the wheel only. Any-event tracking repaints on every move.
    let _ = out.write_all(b"\x1b[?1002l\x1b[?1003l");
    let _ = out.flush();
    let events = Events::start(REFRESH);
    let result = (|| -> Result<()> {
        popup.draw(&mut out)?;
        while !popup.quit {
            let draw = match events.next()? {
                TermEvent::Key(key) => {
                    popup.key(key);
                    true
                }
                TermEvent::Mouse(mouse) => popup.mouse(mouse),
                TermEvent::Resize => true,
                TermEvent::Tick => matches!(popup.mode, Mode::List) && popup.refresh_list(),
            };
            if draw {
                popup.draw(&mut out)?;
            }
        }
        Ok(())
    })();
    let _ = execute!(
        out,
        DisableMouseCapture,
        cursor::Show,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
    if let Some((socket, machine, pane)) = popup.jump.take() {
        focus(ctx, &socket, &machine, &pane);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_parse_with_lists_owners_and_threads() {
        let text = "# Tasks\n\n## Backlog\n- [ ] Write the docs (me)\n- [ ] Fix login (codex-fast@m1) · t-0007\n- [ ] Plain line\n\n## Later\n- [x] Old (agent)\n";
        let tasks = parse_tasks("demo", text);
        assert_eq!(tasks.len(), 4);
        assert_eq!(
            (
                tasks[0].list.as_str(),
                tasks[0].title.as_str(),
                tasks[0].owner.as_str()
            ),
            ("Backlog", "Write the docs", "me")
        );
        assert_eq!(
            (tasks[1].owner.as_str(), tasks[1].thread.as_deref()),
            ("codex-fast@m1", Some("t-0007"))
        );
        assert_eq!(tasks[2].owner, "");
        assert_eq!(
            (tasks[3].list.as_str(), tasks[3].owner.as_str()),
            ("Later", "")
        );
    }

    #[test]
    fn pr_facts_read_like_the_plan() {
        let t = Thread {
            pr: "https://github.com/o/r/pull/4".into(),
            ..Thread::default()
        };
        let s = crate::pr::Summary {
            state: "OPEN".into(),
            review_decision: "APPROVED".into(),
            failing_checks: vec![],
            comment_count: 2,
            commenters: vec![],
            ..Default::default()
        };
        assert_eq!(
            pr_facts(&t, Some(&s)),
            "PR #4 · approved · checks ✓ · 2 comments"
        );
        let failing = crate::pr::Summary {
            failing_checks: vec!["lint".into()],
            comment_count: 1,
            review_decision: String::new(),
            ..s
        };
        assert_eq!(
            pr_facts(&t, Some(&failing)),
            "PR #4 · checks ✗ 1 · 1 comment"
        );
        assert_eq!(pr_facts(&Thread::default(), None), "");
    }

    #[test]
    fn rows_group_threads_by_need_and_sections_have_rows() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        world.thread(&project, world.home.path(), |t| {
            t.last_group = "working".into();
            t.state_line = "working · ~40%".into();
        });
        let second = thread::allocate(&project, |t| {
            t.title = "Second".into();
            t.status = thread::Status::Open;
            t.last_group = "waiting-on-you".into();
        })
        .unwrap();
        std::fs::write(
            thread::home_report_path(&project, &second.id),
            "## Report\nok\n## Next\n- Merge the PR\n",
        )
        .unwrap();
        let needs = build(&world.ctx(), View::NeedsYou, Some("demo"));
        assert!(
            needs.iter().any(|row| {
                row.text.contains("t-0002")
                    && row.text.contains("Second")
                    && row.text.contains("next: 1")
            }),
            "{needs:?}"
        );
        assert!(needs.iter().all(|row| !row.text.contains("working · ~40%")));
        let work = build(&world.ctx(), View::Work, Some("demo"));
        assert!(work.iter().any(|row| row.text.contains("working · ~40%")));
        assert!(work.iter().any(|row| row.text.contains("t-0002")));
        let overview = build(&world.ctx(), View::Overview, Some("demo"));
        assert_eq!(overview[0].text, "2 open · 1 needs you · 0 reviews");
        assert!(overview.iter().any(|row| row.text.contains("coordinator")));
        let all = build(&world.ctx(), View::Overview, None);
        assert_eq!(all[0].text, "2 open · 1 needs you · 0 reviews");
        let settings = build(&world.ctx(), View::Settings, Some("demo"));
        assert!(
            settings
                .iter()
                .any(|r| r.text.contains("max_parallel_threads"))
        );
        assert!(!build(&world.ctx(), View::Tasks, Some("demo")).is_empty());
        std::fs::write(project.dir().join("TASKS.md"), "# Tasks\n\n## Backlog\n- [ ] Fix login (claude) · t-0003\n  Safari drops the cookie.\n  See issue 42.\n- [ ] Docs (me)\n").unwrap();
        let tasks = build(&world.ctx(), View::Tasks, Some("demo"));
        let texts: Vec<(&str, bool)> = tasks.iter().map(|r| (r.text.as_str(), r.header)).collect();
        assert_eq!(
            texts,
            [
                ("Backlog", true),
                ("  Fix login  (claude) · t-0003  ≡", false),
                ("  Docs  (me)", false)
            ]
        );
        let RowKind::Task(task) = &tasks[1].kind else {
            panic!()
        };
        let Mode::Detail { title, lines, .. } = task_detail(task) else {
            panic!()
        };
        assert_eq!(
            (title.as_str(), lines),
            (
                "Fix login",
                vec![
                    "Backlog · claude".to_string(),
                    String::new(),
                    "  Safari drops the cookie.".into(),
                    "  See issue 42.".into()
                ]
            )
        );
        assert!(!build(&world.ctx(), View::Memory, Some("demo")).is_empty());
        assert_eq!(summary(&world.root), "1 project · 1 need you");
    }

    fn rows() -> Vec<PickerRow> {
        let row = |slug: Option<&str>, name: &str| PickerRow {
            slug: slug.map(String::from),
            name: name.into(),
            status: "idle".into(),
        };
        vec![
            row(None, "All projects"),
            row(Some("gtm-ai"), "GTM AI"),
            row(Some("herdr-projects"), "Herdr Projects"),
            row(Some("pi"), "pi"),
        ]
    }

    fn press(picker: &mut Picker, code: KeyCode) -> PickerOutcome {
        picker.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(picker: &mut Picker, text: &str) {
        for c in text.chars() {
            assert_eq!(press(picker, KeyCode::Char(c)), PickerOutcome::Stay);
        }
    }

    fn names(picker: &Picker) -> Vec<&str> {
        picker.visible().iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn the_picker_opens_on_the_current_scope_and_wraps_both_ways() {
        let mut picker = Picker::new(rows(), Some("herdr-projects"), false);
        assert_eq!(picker.selected, 2);
        press(&mut picker, KeyCode::Down);
        assert_eq!(picker.selected, 3);
        press(&mut picker, KeyCode::Char('j'));
        assert_eq!(
            picker.selected, 0,
            "wraps from the last row to All projects"
        );
        press(&mut picker, KeyCode::Up);
        assert_eq!(picker.selected, 3, "wraps from the first row to the last");
        press(&mut picker, KeyCode::Char('k'));
        assert_eq!(
            press(&mut picker, KeyCode::Enter),
            PickerOutcome::Pick(Some("herdr-projects".into()))
        );
        // All projects is the first row, and the scope when there is none.
        let mut all = Picker::new(rows(), None, false);
        assert_eq!(all.selected, 0);
        assert_eq!(press(&mut all, KeyCode::Enter), PickerOutcome::Pick(None));
        // A scope that is not listed (an archived project) starts at the top.
        assert_eq!(Picker::new(rows(), Some("old"), false).selected, 0);
        // Esc closes without a pick; other letters do nothing.
        let mut picker = Picker::new(rows(), Some("pi"), false);
        assert_eq!(press(&mut picker, KeyCode::Char('x')), PickerOutcome::Stay);
        assert_eq!(picker.selected, 3);
        assert_eq!(press(&mut picker, KeyCode::Esc), PickerOutcome::Close);
    }

    #[test]
    fn the_filter_narrows_on_name_and_slug_and_picks_the_highlighted_match() {
        let mut picker = Picker::new(rows(), None, false);
        press(&mut picker, KeyCode::Char('/'));
        assert_eq!(picker.filter.as_deref(), Some(""));
        assert_eq!(names(&picker).len(), 4);
        // Case-insensitive on the name; j and k are text while filtering.
        typed(&mut picker, "HERDR");
        assert_eq!(names(&picker), ["Herdr Projects"]);
        press(&mut picker, KeyCode::Backspace);
        assert_eq!(picker.filter.as_deref(), Some("HERD"));
        // On the slug too.
        let mut picker = Picker::new(rows(), None, true);
        typed(&mut picker, "gtm-");
        assert_eq!(names(&picker), ["GTM AI"]);
        // Several matches: ↓ moves among them (wrapping), ↵ picks.
        let mut picker = Picker::new(rows(), None, true);
        typed(&mut picker, "p");
        assert_eq!(names(&picker), ["All projects", "Herdr Projects", "pi"]);
        assert_eq!(picker.selected, 0);
        press(&mut picker, KeyCode::Down);
        press(&mut picker, KeyCode::Down);
        assert_eq!(
            press(&mut picker, KeyCode::Enter),
            PickerOutcome::Pick(Some("pi".into()))
        );
        let mut picker = Picker::new(rows(), None, true);
        typed(&mut picker, "zz");
        assert_eq!(names(&picker), Vec::<&str>::new());
        assert_eq!(
            press(&mut picker, KeyCode::Enter),
            PickerOutcome::Stay,
            "no match: nothing to pick"
        );
    }

    #[test]
    fn esc_clears_a_typed_filter_first_and_closes_on_the_second_press() {
        let mut picker = Picker::new(rows(), None, true);
        typed(&mut picker, "gtm");
        assert_eq!(press(&mut picker, KeyCode::Esc), PickerOutcome::Stay);
        assert_eq!(picker.filter, None);
        assert_eq!(names(&picker).len(), 4);
        assert_eq!(
            picker.selected, 1,
            "the match stays highlighted in the full list"
        );
        assert_eq!(press(&mut picker, KeyCode::Esc), PickerOutcome::Close);
        // An empty filter has nothing to clear: esc closes at once.
        let mut picker = Picker::new(rows(), None, true);
        assert_eq!(press(&mut picker, KeyCode::Esc), PickerOutcome::Close);
        // So does a filter cleared with backspace.
        let mut picker = Picker::new(rows(), None, true);
        typed(&mut picker, "x");
        press(&mut picker, KeyCode::Backspace);
        assert_eq!(press(&mut picker, KeyCode::Esc), PickerOutcome::Close);
    }

    #[test]
    fn the_settings_section_makes_a_profile_and_allows_it() {
        let world = crate::scenarios::World::new();
        world.project("alpha", "a.sock");
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, None, String::new());
        let key = |popup: &mut Popup, code| popup.key(KeyEvent::new(code, KeyModifiers::NONE));
        popup.view = View::Settings;
        popup.reload();
        // `n`: the form; name, then harness codex (cycled), model, effort, args.
        key(&mut popup, KeyCode::Char('n'));
        for c in "deep".chars() {
            key(&mut popup, KeyCode::Char(c));
        }
        key(&mut popup, KeyCode::Down);
        while !matches!(&popup.mode, Mode::Form { fields, .. } if fields[1].value == "codex") {
            key(&mut popup, KeyCode::Right);
        }
        key(&mut popup, KeyCode::Down);
        for c in "gpt-5.5".chars() {
            key(&mut popup, KeyCode::Char(c));
        }
        key(&mut popup, KeyCode::Down);
        while !matches!(&popup.mode, Mode::Form { fields, .. } if fields[3].value == "high") {
            key(&mut popup, KeyCode::Right);
        }
        key(&mut popup, KeyCode::Down);
        for c in "--search".chars() {
            key(&mut popup, KeyCode::Char(c));
        }
        key(&mut popup, KeyCode::Enter);
        assert!(matches!(popup.mode, Mode::List), "{}", popup.message);
        let config = crate::profiles::load(&ctx.config_dir).unwrap();
        let deep = config.get("deep").unwrap();
        assert_eq!(
            deep.args(),
            [
                "--model",
                "gpt-5.5",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--search"
            ]
        );

        // The all-projects thread list: check `deep` only.
        popup.selected = popup.rows.iter().position(|r| matches!(&r.kind, RowKind::Setting { slug, key, .. } if slug.is_empty() && key == "thread_profiles")).unwrap();
        key(&mut popup, KeyCode::Enter);
        let at = match &popup.mode {
            Mode::Toggle { options, .. } => options.iter().position(|o| o.0 == "deep").unwrap(),
            _ => panic!("not a toggle"),
        };
        for _ in 0..at {
            key(&mut popup, KeyCode::Down);
        }
        key(&mut popup, KeyCode::Char(' '));
        key(&mut popup, KeyCode::Enter);
        assert_eq!(
            project::load_safety(&ctx.config_dir, Path::new(""))
                .unwrap()
                .thread_profiles,
            Some(vec!["deep".to_string()])
        );
    }

    #[test]
    fn slash_and_shift_p_open_the_picker_and_settings_enter_still_scopes() {
        let world = crate::scenarios::World::new();
        world.project("alpha", "a.sock");
        world.project("beta", "a.sock");
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("alpha".into()), String::new());
        let key = |popup: &mut Popup, code| popup.key(KeyEvent::new(code, KeyModifiers::NONE));
        // `/` from the list goes straight into the filter; j is text there.
        key(&mut popup, KeyCode::Char('/'));
        assert!(matches!(&popup.mode, Mode::Projects(p) if p.filter.as_deref() == Some("")));
        for c in "bej".chars() {
            key(&mut popup, KeyCode::Char(c));
        }
        assert!(
            matches!(&popup.mode, Mode::Projects(p) if p.filter.as_deref() == Some("bej") && p.visible().is_empty())
        );
        key(&mut popup, KeyCode::Backspace);
        key(&mut popup, KeyCode::Enter);
        assert!(matches!(popup.mode, Mode::List));
        assert_eq!(popup.scope.as_deref(), Some("beta"));
        // P opens on the current scope; esc leaves it unchanged.
        key(&mut popup, KeyCode::Char('P'));
        assert!(matches!(&popup.mode, Mode::Projects(p) if p.filter.is_none() && p.selected == 2));
        key(&mut popup, KeyCode::Up);
        key(&mut popup, KeyCode::Up);
        key(&mut popup, KeyCode::Esc);
        assert_eq!(popup.scope.as_deref(), Some("beta"));
        key(&mut popup, KeyCode::Char('P'));
        key(&mut popup, KeyCode::Up);
        key(&mut popup, KeyCode::Up);
        key(&mut popup, KeyCode::Enter);
        assert_eq!(popup.scope, None);
        // The settings rows of all projects: ↵ on a project still scopes to it.
        popup.view = View::Settings;
        popup.reload();
        // Profile rows come first; the projects follow.
        popup.selected = popup
            .rows
            .iter()
            .position(|r| matches!(r.kind, RowKind::Project { .. }))
            .unwrap();
        key(&mut popup, KeyCode::Enter);
        assert_eq!(popup.scope.as_deref(), Some("alpha"));
    }

    #[test]
    fn yolo_toggles_from_the_settings_tab_per_project_and_for_all_projects() {
        let world = crate::scenarios::World::new();
        let alpha = world.project("alpha", "a.sock");
        let beta = world.project("beta", "a.sock");
        let ctx = world.ctx();
        let yolo = |p: &Project| p.safety(&ctx.config_dir).unwrap().yolo;
        let mut popup = Popup::new(&ctx, Some("alpha".into()), String::new());
        let key = |popup: &mut Popup, code| popup.key(KeyEvent::new(code, KeyModifiers::NONE));
        popup.view = View::Settings;
        popup.reload();
        assert!(
            popup
                .rows
                .iter()
                .any(|r| r.header && r.text.starts_with("safety"))
        );
        // Y asks before turning yolo on; n leaves it off.
        key(&mut popup, KeyCode::Char('Y'));
        assert!(
            matches!(&popup.mode, Mode::Confirm { question, .. } if question.starts_with("Yolo for alpha?"))
        );
        key(&mut popup, KeyCode::Char('n'));
        assert!(!yolo(&alpha));
        key(&mut popup, KeyCode::Char('Y'));
        key(&mut popup, KeyCode::Char('y'));
        assert!(yolo(&alpha) && !yolo(&beta), "only alpha");
        assert!(popup.message.contains("restarted"), "{}", popup.message);
        assert_eq!(alpha.safety(&ctx.config_dir).unwrap().start_threads, "auto");
        // Turning it off needs no question.
        key(&mut popup, KeyCode::Char('Y'));
        assert!(matches!(popup.mode, Mode::List) && !yolo(&alpha));

        // Unscoped, the rows are the all-projects defaults: ↵ on yolo picks.
        popup.scope = None;
        popup.reload();
        popup.selected = popup
            .rows
            .iter()
            .position(
                |r| matches!(&r.kind, RowKind::Safety { slug: None, key, .. } if key == "yolo"),
            )
            .unwrap();
        key(&mut popup, KeyCode::Enter);
        assert!(matches!(&popup.mode, Mode::Pick { options, .. } if options == &["on", "off"]));
        key(&mut popup, KeyCode::Enter);
        assert!(yolo(&beta), "beta inherits the default");
        assert!(!yolo(&alpha), "alpha keeps its own off");
        // An argument row edits as text; empty means none.
        popup.selected = popup
            .rows
            .iter()
            .position(
                |r| matches!(&r.kind, RowKind::Safety { key, .. } if key == "thread_agent_args"),
            )
            .unwrap();
        key(&mut popup, KeyCode::Enter);
        for c in "--x".chars() {
            key(&mut popup, KeyCode::Char(c));
        }
        key(&mut popup, KeyCode::Enter);
        assert_eq!(
            beta.safety(&ctx.config_dir).unwrap().thread_agent_args,
            ["--x"]
        );
    }

    #[test]
    fn picker_rows_start_with_all_projects_and_leave_out_archived_ones() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        world.thread(&project, world.home.path(), |t| {
            t.last_group = "waiting-on-you".into()
        });
        world.project("old", "a.sock");
        crate::lifecycle::set_status(&world.ctx(), "old", Status::Archived).ok();
        let rows = picker_rows(&world.root);
        let slugs: Vec<Option<&str>> = rows.iter().map(|r| r.slug.as_deref()).collect();
        assert_eq!(slugs, [None, Some("demo")]);
        assert_eq!(rows[0].status, "1 project · 1 need you");
        assert_eq!(rows[1].status, "1 need you");
    }

    fn down(column: usize, row: usize) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: column as u16,
            row: row as u16,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn row_y(popup: &Popup, index: usize) -> usize {
        let (_, height) = screen_size();
        let (body_top, body_height) = body_window(height);
        body_top + index - popup.list_start(body_height)
    }

    fn selected_thread(popup: &Popup) -> Option<String> {
        match popup.current().map(|row| &row.kind) {
            Some(RowKind::Thread(thread)) => Some(thread.thread.id.clone()),
            _ => None,
        }
    }

    fn on_work(popup: &mut Popup) {
        popup.set_view(View::Work);
    }

    fn two_threads(world: &crate::scenarios::World, project: &Project) -> (String, String) {
        let first = world.thread(project, world.home.path(), |thread| {
            thread.title = "First".into();
            thread.last_group = "waiting-on-you".into();
            thread.pane_id = "w2:p1".into();
        });
        let second = thread::allocate(project, |thread| {
            thread.title = "Second".into();
            thread.status = thread::Status::Open;
            thread.last_group = "working".into();
            thread.pane_id = "w2:p2".into();
        })
        .unwrap();
        (first.id, second.id)
    }

    #[test]
    fn popup_g_digit_opens_tasks_then_work_and_keys_still_move() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        two_threads(&world, &project);
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        let key = |popup: &mut Popup, code| popup.key(KeyEvent::new(code, KeyModifiers::NONE));
        key(&mut popup, KeyCode::Char('g'));
        key(&mut popup, KeyCode::Char('6'));
        assert_eq!(popup.view, View::Tasks);
        key(&mut popup, KeyCode::Char('g'));
        key(&mut popup, KeyCode::Char('3'));
        assert_eq!(popup.view, View::Work);
        let before = popup.selected;
        key(&mut popup, KeyCode::Down);
        assert_ne!(popup.selected, before);
    }

    #[test]
    fn popup_mouse_row_click_selects_and_activation_matches_enter() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let (_, second) = two_threads(&world, &project);
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        on_work(&mut popup);
        let index = popup
            .rows
            .iter()
            .position(
                |row| matches!(&row.kind, RowKind::Thread(thread) if thread.thread.id == second),
            )
            .unwrap();
        assert_ne!(popup.selected, index);
        popup.mouse(down(1, row_y(&popup, index)));
        assert_eq!(popup.selected, index);
        assert!(!popup.quit);
        assert!(popup.jump.is_none());
        let mut keys = Popup::new(&ctx, Some("demo".into()), String::new());
        on_work(&mut keys);
        keys.selected = index;
        keys.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        popup.mouse(down(1, row_y(&popup, index)));
        assert_eq!(popup.jump, keys.jump);
        assert_eq!(popup.quit, keys.quit);
        assert!(popup.quit);
    }

    #[test]
    fn popup_mouse_wheel_moves_like_up_and_down() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        two_threads(&world, &project);
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        on_work(&mut popup);
        let start = popup.selected;
        popup.mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let wheeled = popup.selected;
        popup.selected = start;
        popup.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(popup.selected, wheeled);
        assert_ne!(start, wheeled);
        popup.mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(popup.selected, start);
    }

    #[test]
    fn popup_mouse_move_does_not_repaint() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        two_threads(&world, &project);
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        let selected = popup.selected;
        let changed = popup.mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!changed);
        assert_eq!(popup.selected, selected);
        assert!(!popup.quit);
    }

    #[test]
    fn popup_mouse_heading_click_does_not_select_or_act() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        two_threads(&world, &project);
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        let header = popup.rows.iter().position(|row| row.header).unwrap();
        let selected = popup.selected;
        popup.mouse(down(0, row_y(&popup, header)));
        assert_eq!(popup.selected, selected);
        assert!(!popup.quit);
        assert!(popup.jump.is_none());
        assert!(matches!(popup.mode, Mode::List));
    }

    #[test]
    fn popup_mouse_reload_keeps_the_selected_thread() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let (_, second) = two_threads(&world, &project);
        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        on_work(&mut popup);
        popup.selected = popup
            .rows
            .iter()
            .position(
                |row| matches!(&row.kind, RowKind::Thread(thread) if thread.thread.id == second),
            )
            .unwrap();
        thread::allocate(&project, |thread| {
            thread.title = "Earlier".into();
            thread.status = thread::Status::Open;
            thread.last_group = "waiting-on-you".into();
            thread.pane_id = "w2:p3".into();
        })
        .unwrap();
        popup.reload();
        assert_eq!(selected_thread(&popup).as_deref(), Some(second.as_str()));
    }

    #[test]
    fn popup_state_thread_line_keeps_id_state_pr_and_next_at_80_columns() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let thread = world.thread(&project, world.home.path(), |thread| {
            thread.title = "T".repeat(200);
            thread.last_group = "waiting-on-you".into();
            thread.last_state = "blocked".into();
            thread.pr = "https://github.com/o/r/pull/17".into();
            thread.status = thread::Status::Open;
        });
        std::fs::write(
            thread::extra_next_path(&project, &thread.id),
            "- one\n- two\n",
        )
        .unwrap();
        let rows = build(&world.ctx(), View::NeedsYou, Some("demo"));
        let row = rows
            .iter()
            .find(|row| matches!(&row.kind, RowKind::Thread(found) if found.thread.id == thread.id))
            .unwrap();
        let painted = paint_list_row(row, true, 80);
        assert!(painted.chars().count() <= 80, "{painted}");
        assert!(painted.contains(&thread.id), "{painted}");
        assert!(painted.contains("needs you"), "{painted}");
        assert!(painted.contains("blocked"), "{painted}");
        assert!(painted.contains("PR #17"), "{painted}");
        assert!(painted.contains("next: 2"), "{painted}");
        assert!(!painted.contains(&"T".repeat(200)), "{painted}");
    }

    #[test]
    fn popup_state_gone_primary_is_unavailable_and_not_another_agent() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        project
            .update_coordinator(|coordinator| {
                coordinator.socket = world
                    .home
                    .path()
                    .join("missing-primary.sock")
                    .display()
                    .to_string();
                coordinator.pane_id = "w1:p1".into();
            })
            .unwrap();
        crate::coordinator::save_live(
            &project,
            &[crate::coordinator::LivePane {
                name: "other-lead".into(),
                pane_id: "w9:p9".into(),
                ..crate::coordinator::LivePane::default()
            }],
        )
        .unwrap();
        let rows = build(&world.ctx(), View::Overview, Some("demo"));
        let text = rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("unavailable"), "{text}");
        assert!(!text.contains("other-lead"), "{text}");
    }

    #[test]
    fn needs_you_omits_a_failed_thread_and_places_keep_a_null_repository() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        world.thread(&project, world.home.path(), |thread| {
            thread.title = "Dead".into();
            thread.status = thread::Status::Failed;
            thread.last_group = "waiting-on-you".into();
        });
        thread::allocate(&project, |thread| {
            thread.title = "Live".into();
            thread.status = thread::Status::Open;
            thread.last_group = "waiting-on-you".into();
        })
        .unwrap();

        let needs = build(&world.ctx(), View::NeedsYou, Some("demo"));
        let needs_text = needs
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(needs_text.contains("Live"), "{needs_text}");
        assert!(!needs_text.contains("Dead"), "{needs_text}");

        let work = build(&world.ctx(), View::Work, Some("demo"));
        let dead = work
            .iter()
            .find(|row| row.text.contains("Dead"))
            .expect("failed thread is a work row");
        assert!(dead.text.contains("failed"), "{}", dead.text);
        assert!(!dead.text.contains("needs you"), "{}", dead.text);

        let store = crate::store::Store::open(&project.root).unwrap();
        let row = store.project_by_slug("demo").unwrap().unwrap();
        store
            .reconcile_repos(&row.id, &[("/tmp/demo-repo", "")])
            .unwrap();
        let repo_id = store
            .repos(&row.id)
            .unwrap()
            .into_iter()
            .find(|repo| repo.path == "/tmp/demo-repo")
            .unwrap()
            .id;
        store
            .ensure_workspace(
                &row.id,
                &crate::store::WorkspaceDraft {
                    kind: "checkout".into(),
                    cwd: "/tmp/demo-repo".into(),
                    worktree_root: String::new(),
                    branch: "main".into(),
                    base_ref: String::new(),
                    ownership: "legacy".into(),
                    environment_id: crate::ids::ENV_LOCAL.into(),
                    repository_id: Some(repo_id),
                },
            )
            .unwrap();
        let dir = project.dir().to_string_lossy().into_owned();
        store
            .ensure_workspace(
                &row.id,
                &crate::store::WorkspaceDraft {
                    kind: "directory".into(),
                    cwd: dir.clone(),
                    worktree_root: String::new(),
                    branch: String::new(),
                    base_ref: String::new(),
                    ownership: "legacy".into(),
                    environment_id: crate::ids::ENV_LOCAL.into(),
                    repository_id: None,
                },
            )
            .unwrap();

        let places = build(&world.ctx(), View::Places, Some("demo"));
        let places_text: Vec<&str> = places.iter().map(|row| row.text.as_str()).collect();
        let directory = places_text
            .iter()
            .position(|text| text.contains("Project directory"))
            .unwrap();
        let repo = places_text
            .iter()
            .position(|text| text.contains("/tmp/demo-repo"))
            .unwrap();
        assert!(repo < directory, "{places_text:?}");
        assert!(
            places_text
                .iter()
                .skip(directory + 1)
                .any(|text| text.contains(&dir)),
            "{places_text:?}"
        );

        assert_eq!(column_widths(80, false), (0, 80, 0));
        assert_eq!(column_widths(70, false).0, 0);
        let (project_120, _, inspector_120) = column_widths(120, false);
        assert!(project_120 > 0 && inspector_120 > 0);
        let (project_160, _, inspector_160) = column_widths(160, false);
        assert!(project_160 > 0 && inspector_160 > 0);
        assert_eq!(column_widths(160, true).0, 0);

        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        assert_eq!(popup.view, View::Overview);
        popup.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!popup.quit);
        popup.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        popup.key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(popup.view, View::NeedsYou);
    }

    #[test]
    fn density_screen_counts_places_and_the_narrow_inspector() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let harnesses = ["claude", "codex", "cursor", "gemini"];
        for n in 0..24 {
            thread::allocate(&project, |thread| {
                thread.title = format!("Row {n}");
                thread.status = if (5..7).contains(&n) {
                    thread::Status::Failed
                } else {
                    thread::Status::Open
                };
                thread.last_group = if n < 5 || (5..7).contains(&n) {
                    "waiting-on-you".into()
                } else if n == 7 || n == 8 {
                    "ready-for-review".into()
                } else if n == 9 {
                    "landing".into()
                } else {
                    "working".into()
                };
                thread.agent = harnesses[n % 4].into();
                if (7..10).contains(&n) {
                    thread.pr = format!("https://github.com/o/r/pull/{}", n - 6);
                }
            })
            .unwrap();
        }
        std::fs::write(
            project.dir().join("TASKS.md"),
            "# Tasks\n\n## Backlog\n- [ ] Density task (me)\n",
        )
        .unwrap();

        let store = crate::store::Store::open(&project.root).unwrap();
        let row = store.project_by_slug("demo").unwrap().unwrap();
        let session = store.primary_session(&row.id).unwrap().unwrap();
        store.mark_session_stale(&session.id).unwrap();
        store
            .reconcile_repos(
                &row.id,
                &[
                    ("/repo/a", "m1"),
                    ("/repo/b", "m2"),
                    ("/repo/c", "m3"),
                    ("/repo/d", "m1"),
                ],
            )
            .unwrap();
        let repos = store.repos(&row.id).unwrap();
        let repo_id = |path: &str| {
            repos
                .iter()
                .find(|repo| repo.path == path)
                .unwrap()
                .id
                .clone()
        };
        let gone = store.environment_for_label("gone-1").unwrap();
        assert_eq!(gone.state, "unresolved");
        let place = |repository: Option<String>, cwd: &str, env: &str| {
            store
                .ensure_workspace(
                    &row.id,
                    &crate::store::WorkspaceDraft {
                        kind: "checkout".into(),
                        cwd: cwd.into(),
                        worktree_root: String::new(),
                        branch: "main".into(),
                        base_ref: String::new(),
                        ownership: "legacy".into(),
                        environment_id: env.into(),
                        repository_id: repository,
                    },
                )
                .unwrap();
        };
        let local = crate::ids::ENV_LOCAL;
        place(Some(repo_id("/repo/a")), "/repo/a/w0", local);
        place(Some(repo_id("/repo/a")), "/repo/a/w1", local);
        place(Some(repo_id("/repo/a")), "/repo/a/gone", &gone.id);
        place(Some(repo_id("/repo/b")), "/repo/b/w0", local);
        place(Some(repo_id("/repo/b")), "/repo/b/w1", local);
        place(Some(repo_id("/repo/b")), "/repo/b/w2", local);
        place(Some(repo_id("/repo/c")), "/repo/c/w0", local);
        place(Some(repo_id("/repo/c")), "/repo/c/w1", local);
        place(Some(repo_id("/repo/d")), "/repo/d/w0", local);
        place(Some(repo_id("/repo/d")), "/repo/d/gone", &gone.id);
        let dir = project.dir().to_string_lossy().into_owned();
        place(None, &dir, local);
        place(None, "/proj/loose", local);

        let overview = build(&world.ctx(), View::Overview, Some("demo"));
        assert_eq!(overview[0].text, "24 open · 5 needs you · 3 reviews");
        assert!(overview.iter().any(|row| row.text.contains("stale")));

        let needs = build(&world.ctx(), View::NeedsYou, Some("demo"));
        let need_rows: Vec<_> = needs
            .iter()
            .filter(|row| matches!(row.kind, RowKind::Thread(_)))
            .collect();
        assert_eq!(need_rows.len(), 5);
        assert!(need_rows.iter().all(|row| !row.text.contains("failed")));
        assert!(needs.iter().all(|row| !row.text.contains("Density task")));

        let work = build(&world.ctx(), View::Work, Some("demo"));
        let work_rows: Vec<_> = work
            .iter()
            .filter(|row| matches!(row.kind, RowKind::Thread(_)))
            .collect();
        assert_eq!(work_rows.len(), 24);
        let failed: Vec<_> = work_rows
            .iter()
            .filter(|row| row.text.contains("failed"))
            .collect();
        assert_eq!(failed.len(), 2);
        assert!(failed.iter().all(|row| !row.text.contains("needs you")));
        for name in harnesses {
            assert!(work_rows.iter().any(|row| row.text.contains(name)));
        }

        let reviews = build(&world.ctx(), View::Reviews, Some("demo"));
        let review_rows: Vec<_> = reviews
            .iter()
            .filter(|row| matches!(row.kind, RowKind::Thread(_)))
            .collect();
        assert_eq!(review_rows.len(), 3);
        assert!(review_rows.iter().all(|row| {
            (row.text.contains("review") || row.text.contains("landing"))
                && row.text.contains("PR #")
        }));
        let RowKind::Thread(sample) = &review_rows[0].kind else {
            panic!("review row");
        };
        let lines = inspector_lines(review_rows[0]);
        assert_eq!(lines[0], sample.thread.id);
        assert!(lines.iter().any(|line| line == &sample.thread.pr));
        assert!(lines.iter().any(|line| line == &sample.thread.agent));

        let places = build(&world.ctx(), View::Places, Some("demo"));
        let places_text: Vec<&str> = places.iter().map(|row| row.text.as_str()).collect();
        let directory = places_text
            .iter()
            .position(|text| *text == "Project directory")
            .unwrap();
        for path in ["/repo/a", "/repo/b", "/repo/c", "/repo/d"] {
            let at = places_text
                .iter()
                .position(|text| text.contains(path))
                .unwrap();
            assert!(at < directory, "{path} {places_text:?}");
        }
        assert!(
            places_text
                .iter()
                .skip(directory + 1)
                .any(|text| text.contains(&dir))
        );
        assert!(
            places_text
                .iter()
                .skip(directory + 1)
                .any(|text| text.contains("/proj/loose"))
        );
        assert_eq!(
            places_text
                .iter()
                .filter(|text| text.contains("unresolved"))
                .count(),
            2
        );
        assert!(
            places
                .iter()
                .all(|row| !matches!(row.kind, RowKind::Thread(_)))
        );

        let ctx = world.ctx();
        let mut popup = Popup::new(&ctx, Some("demo".into()), String::new());
        let key = |popup: &mut Popup, code| popup.key(KeyEvent::new(code, KeyModifiers::NONE));
        let scope = popup.scope.clone();
        key(&mut popup, KeyCode::Char('g'));
        key(&mut popup, KeyCode::Char('3'));
        assert_eq!(popup.view, View::Work);
        key(&mut popup, KeyCode::Char('i'));
        assert!(matches!(popup.mode, Mode::Detail { .. }));
        key(&mut popup, KeyCode::Esc);
        assert!(matches!(popup.mode, Mode::List));
        assert!(!popup.quit);
        key(&mut popup, KeyCode::Char('x'));
        assert!(matches!(popup.mode, Mode::Confirm { .. }));
        key(&mut popup, KeyCode::Esc);
        assert!(matches!(popup.mode, Mode::List));
        key(&mut popup, KeyCode::Char('r'));
        assert!(matches!(popup.mode, Mode::Pick { .. }));
        key(&mut popup, KeyCode::Esc);
        key(&mut popup, KeyCode::Char('t'));
        assert!(matches!(
            popup.mode,
            Mode::Start {
                phase: StartPhase::Kind,
                ..
            }
        ));
        key(&mut popup, KeyCode::Esc);
        assert!(matches!(popup.mode, Mode::List));
        assert_eq!(popup.message, "cancelled");
        key(&mut popup, KeyCode::Char('l'));
        assert!(popup.inspect);
        assert_eq!(popup.scope, scope);
        assert!(!popup.quit);
        key(&mut popup, KeyCode::Esc);
        assert!(!popup.inspect);
        assert!(!popup.quit);
        key(&mut popup, KeyCode::Char('h'));
        assert_eq!(popup.scope, scope);
        assert!(!popup.inspect);
        key(&mut popup, KeyCode::Char('q'));
        assert!(!popup.quit);
        key(&mut popup, KeyCode::Esc);
        assert!(popup.quit);
    }
}
