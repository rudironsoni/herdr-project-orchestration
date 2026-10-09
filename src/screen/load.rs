use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::project::{self, Project};
use crate::store::Store;
use crate::thread::{self, Thread};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub projects: Vec<Card>,
    pub failed: Vec<FailedImport>,
    pub needs: Vec<NeedsRow>,
    pub inbox: Vec<InboxRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedImport {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedsRow {
    pub project_id: String,
    pub slug: String,
    pub thread_id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxRow {
    pub project_id: String,
    pub slug: String,
    pub id: String,
    pub summary: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub lifecycle: String,
    pub availability: String,
    pub binding: String,
    pub setup_incomplete: bool,
    pub goal: String,
    pub coordinator: String,
    pub threads: Vec<ThreadLine>,
    pub tasks: Vec<TaskLine>,
    pub library: Vec<LibraryLine>,
    pub prs: Vec<PrLine>,
    pub routines: Vec<RoutineLine>,
    pub resources: Vec<ResourceLine>,
    pub safety: Vec<SafetyLine>,
    pub reports: Vec<String>,
    pub operations: Vec<String>,
    pub recovery: Vec<String>,
    pub running: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadLine {
    pub id: String,
    pub title: String,
    pub status: String,
    pub pane_id: String,
    pub files: Vec<String>,
    pub machine: String,
    pub pr: String,
    pub unavailable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskLine {
    pub title: String,
    pub list: String,
    pub owner: String,
    pub thread: Option<String>,
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryLine {
    pub label: String,
    pub path: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrLine {
    pub thread_id: String,
    pub title: String,
    pub reference: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutineLine {
    pub name: String,
    pub enabled: bool,
    pub schedule: String,
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafetyLine {
    pub key: String,
    pub value: String,
    pub source: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLine {
    pub kind: String,
    pub label: String,
}

pub fn load(root: &Path, config_dir: &Path) -> Result<Snapshot> {
    let index = project::import_and_list(root)?;
    let profiles = crate::profiles::load(config_dir)
        .map(|config| config.listed(&[]))
        .unwrap_or_default();
    let profile_names: Vec<String> = profiles.into_iter().map(|profile| profile.name).collect();
    let mut projects = Vec::new();
    let mut needs = Vec::new();
    let mut inbox = Vec::new();
    for row in index.projects {
        let project = Project::load(root, &row.slug)?;
        let card = card_for(&project, &row, &profile_names, config_dir)?;
        for thread in &card.threads {
            if needs_attention(&project, &thread.id) {
                needs.push(NeedsRow {
                    project_id: card.id.clone(),
                    slug: card.slug.clone(),
                    thread_id: thread.id.clone(),
                    title: thread.title.clone(),
                });
            }
        }
        for item in crate::inbox::unhandled(&project) {
            inbox.push(InboxRow {
                project_id: card.id.clone(),
                slug: card.slug.clone(),
                id: item.id,
                summary: item.summary,
                body: item.body,
            });
        }
        projects.push(card);
    }
    let failed = index
        .failed_imports
        .into_iter()
        .map(|(path, message)| FailedImport { path, message })
        .collect();
    Ok(Snapshot {
        projects,
        failed,
        needs,
        inbox,
    })
}

fn card_for(
    project: &Project,
    row: &crate::store::ProjectRow,
    profiles: &[String],
    config_dir: &Path,
) -> Result<Card> {
    let (name, goal, repos, thread_profile, coordinator_profile) = match project.read_project_md() {
        Ok((settings, _)) => (
            settings.name,
            settings.goal,
            settings.repos,
            settings.thread_profile,
            settings.coordinator_profile,
        ),
        Err(_) => (
            row.slug.clone(),
            String::new(),
            Vec::new(),
            String::new(),
            String::new(),
        ),
    };
    let store = Store::open(&project.root)?;
    let fresh = store
        .project_by_slug(&row.slug)?
        .unwrap_or_else(|| row.clone());
    let threads = thread::list(project);
    let thread_lines: Vec<ThreadLine> = threads
        .iter()
        .map(|thread| thread_line(project, thread))
        .collect();
    let prs = thread_lines
        .iter()
        .filter(|thread| !thread.pr.is_empty())
        .map(|thread| PrLine {
            thread_id: thread.id.clone(),
            title: thread.title.clone(),
            reference: thread.pr.clone(),
        })
        .collect();
    let tasks_text = std::fs::read_to_string(project.dir().join("TASKS.md")).unwrap_or_default();
    let tasks = crate::tasks::parse(&tasks_text)
        .into_iter()
        .map(|task| TaskLine {
            owner: task.owner.to_string(),
            list: task.list,
            title: task.title,
            thread: task.thread,
            notes: task.description,
        })
        .collect();
    let (routines, _broken) = crate::routine::load_all(project);
    let routines = routines
        .into_iter()
        .map(|routine| RoutineLine {
            name: routine.name,
            enabled: routine.enabled,
            schedule: routine.schedule_text,
            prompt: routine.prompt,
        })
        .collect();
    let mut resources = Vec::new();
    for repo in &repos {
        let machine = repo.machine.as_deref().unwrap_or("local");
        resources.push(ResourceLine {
            kind: "repository".into(),
            label: format!("{} ({machine})", repo.path),
        });
    }
    if let Ok(spaces) = store.list_workspaces(&fresh.id) {
        for space in spaces {
            resources.push(ResourceLine {
                kind: "workspace".into(),
                label: format!("{} {}", space.cwd, space.availability),
            });
        }
    }
    let mut machines: Vec<String> = repos
        .iter()
        .filter_map(|repo| repo.machine.clone())
        .collect();
    for thread in &thread_lines {
        if !thread.machine.is_empty() {
            machines.push(thread.machine.clone());
        }
    }
    machines.sort();
    machines.dedup();
    for machine in machines {
        resources.push(ResourceLine {
            kind: "machine".into(),
            label: machine,
        });
    }
    if !thread_profile.is_empty() {
        resources.push(ResourceLine {
            kind: "profile".into(),
            label: format!("thread profile {thread_profile}"),
        });
    }
    if !coordinator_profile.is_empty() {
        resources.push(ResourceLine {
            kind: "profile".into(),
            label: format!("coordinator profile {coordinator_profile}"),
        });
    }
    for name in profiles {
        resources.push(ResourceLine {
            kind: "profile".into(),
            label: format!("harness profile {name}"),
        });
    }
    let reports = thread_lines
        .iter()
        .map(|thread| report_line(project, &thread.id))
        .collect();
    let running = thread_lines
        .iter()
        .filter(|thread| thread.status == "open" && !thread.pane_id.is_empty())
        .map(|thread| format!("pane {} may still be running", thread.pane_id))
        .collect();
    Ok(Card {
        id: fresh.id.clone(),
        slug: fresh.slug.clone(),
        name,
        lifecycle: fresh.lifecycle.clone(),
        availability: fresh.availability.clone(),
        binding: if fresh.primary_session_id.is_some() {
            "coordinator bound".into()
        } else {
            "no coordinator binding".into()
        },
        setup_incomplete: project::setup_incomplete(project).unwrap_or(false),
        goal,
        coordinator: coordinator_line(project),
        library: library(project),
        operations: operation_lines(&store, &fresh.id)?,
        recovery: recovery_lines(project)?,
        threads: thread_lines,
        tasks,
        prs,
        routines,
        resources,
        safety: safety_lines(config_dir, project),
        reports,
        running,
    })
}

fn safety_lines(config_dir: &Path, project: &Project) -> Vec<SafetyLine> {
    let target = crate::safety::Target::Project(project.clone());
    crate::safety::rows(config_dir, &target)
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|row| SafetyLine {
            key: row.key.to_string(),
            value: row.value,
            source: row.source.to_string(),
            note: row.note,
        })
        .collect()
}

fn thread_line(project: &Project, thread: &Thread) -> ThreadLine {
    let status = match thread.status {
        thread::Status::Starting => "starting",
        thread::Status::Open => "open",
        thread::Status::Failed => "failed",
        thread::Status::Resolved => "resolved",
    };
    ThreadLine {
        unavailable: thread.status == thread::Status::Failed || thread.pane_id.is_empty(),
        id: thread.id.clone(),
        title: thread.title.clone(),
        status: status.into(),
        pane_id: thread.pane_id.clone(),
        machine: thread.machine.clone(),
        pr: thread.pr.clone(),
        files: thread_files(project, &thread.id),
    }
}

fn thread_files(project: &Project, id: &str) -> Vec<String> {
    let mut files = Vec::new();
    for dir in [
        project.dir().join("library").join(id),
        project.dir().join("uploads"),
    ] {
        let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file())
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        files.extend(found.into_iter().map(|path| path.display().to_string()));
    }
    files
}

fn needs_attention(project: &Project, id: &str) -> bool {
    let Ok(thread) = thread::load(project, id) else {
        return false;
    };
    if thread.status == thread::Status::Failed {
        return false;
    }
    let live = thread::Live {
        pane_exists: !thread.pane_id.is_empty(),
        agent_state: None,
        state_secs: 0,
        self_report: None,
        report_age_secs: 0,
    };
    crate::sidebar::needs_you(thread::group(&thread, &live, jiff::Timestamp::now()))
}

fn coordinator_line(project: &Project) -> String {
    match project.coordinator() {
        None => "coordinator: none recorded".into(),
        Some(record) => {
            let socket = if record.socket.is_empty() {
                "no socket"
            } else if Path::new(&record.socket).exists() {
                "socket present"
            } else {
                "stale: socket missing"
            };
            let name = if record.agent_name.is_empty() {
                "unnamed"
            } else {
                record.agent_name.as_str()
            };
            let pane = if record.pane_id.is_empty() {
                "none"
            } else {
                record.pane_id.as_str()
            };
            let profile = if record.profile.is_empty() {
                record.agent.as_str()
            } else {
                record.profile.as_str()
            };
            format!("coordinator: {socket} pane {pane} {name} profile {profile}")
        }
    }
}

fn report_line(project: &Project, id: &str) -> String {
    let path = thread::home_report_path(project, id);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let line = text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            format!("report {id}: {line}")
        }
        Err(_) => format!("report {id}: no report file"),
    }
}

fn library(project: &Project) -> Vec<LibraryLine> {
    let mut out = Vec::new();
    for name in [
        "PROJECT.md",
        "MEMORY.md",
        "TASKS.md",
        "AGENTS.md",
        "CLAUDE.md",
    ] {
        push_file(&mut out, &project.dir().join(name), name);
    }
    push_dir(&mut out, &project.dir().join("memory"), "memory");
    push_dir(&mut out, &project.dir().join("threads"), "threads");
    out
}

fn push_dir(out: &mut Vec<LibraryLine>, dir: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    names.sort();
    for path in names {
        let label = format!(
            "{prefix}/{}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
        );
        push_file(out, &path, &label);
    }
}

fn push_file(out: &mut Vec<LibraryLine>, path: &Path, label: &str) {
    if path.is_file() {
        let text: String = std::fs::read_to_string(path)
            .unwrap_or_default()
            .chars()
            .take(2000)
            .collect();
        out.push(LibraryLine {
            label: label.into(),
            path: path.display().to_string(),
            text,
        });
    }
}

fn operation_lines(store: &Store, project_id: &str) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for kind in [
        "create_project",
        "write_defaults",
        "write_priming",
        "thread_start",
        "open_coordinator",
    ] {
        if let Some(op) = store.latest_kind(project_id, kind)? {
            let target = serde_json::from_str::<serde_json::Value>(&op.payload)
                .ok()
                .and_then(|value| value.get("target_id")?.as_str().map(str::to_string))
                .unwrap_or_default();
            lines.push(format!("operation {} {kind} {target}", op.status));
        }
    }
    Ok(lines)
}

fn recovery_lines(project: &Project) -> Result<Vec<String>> {
    Ok(match crate::threads::recover_start(project)? {
        crate::threads::StartRecovery::None => Vec::new(),
        crate::threads::StartRecovery::Recorded { id, operation_id } => {
            vec![format!("recorded start {id} operation {operation_id}")]
        }
        crate::threads::StartRecovery::Unresolved {
            id,
            operation_id,
            reason,
        } => {
            let mut lines = vec![
                format!("unresolved recovery {id}"),
                format!("operation {operation_id}"),
            ];
            lines.extend(fold_line(&reason, 48));
            lines
        }
    })
}

fn fold_line(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
            continue;
        }
        if current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

pub fn thread_effect(kind: &str) -> &'static str {
    match kind {
        "worktree" => "creates a branch and a worktree through thread start",
        "tab" => "opens a tab in the project workspace and does not create a worktree",
        "checkout" => "uses the repository checkout and does not create a worktree",
        "adopted" => "adopts an existing pane with thread adopt and does not create a worktree",
        _ => "pick a thread kind",
    }
}
