//! Reads the 0.2.34 operational files once. Runtime code does not parse them.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::project::{Coordinator, Project, ProjectState, Status};
use crate::store::Store;

pub fn operational_paths(project: &Project) -> (PathBuf, PathBuf) {
    let state = project.state_dir();
    (state.join("project.json"), state.join("coordinator.json"))
}

pub fn read_project_state(state_path: &Path, coordinator_path: &Path) -> Result<(String, Vec<String>, Option<Coordinator>)> {
    let legacy = if state_path.is_file() {
        match crate::project::read_json::<ProjectState>(state_path) {
            Some(state) => state,
            None => bail!("project.json does not parse"),
        }
    } else {
        ProjectState::default()
    };
    let coordinator = if coordinator_path.is_file() {
        match crate::project::read_json::<Coordinator>(coordinator_path) {
            Some(record) => Some(record),
            None => bail!("coordinator.json does not parse"),
        }
    } else {
        None
    };
    let lifecycle = match legacy.status {
        Status::Paused => "paused",
        Status::Archived => "archived",
        Status::Active => "active",
    };
    Ok((lifecycle.to_string(), legacy.former_slugs, coordinator))
}

/// Reads `threads/*.toml` for one project directory. Returns the combined
/// parse errors. The caller records them on that directory's import row.
pub fn read_thread_files(store: &Store, project: &Project, project_id: &str) -> Result<String> {
    let mut errors = Vec::new();
    let threads = project.dir().join("threads");
    if let Ok(entries) = std::fs::read_dir(threads) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".toml").map(str::to_string) else {
                continue;
            };
            if crate::thread::validate_id(&id).is_err() {
                errors.push(format!("{name}: not a thread id"));
                continue;
            }
            if store.thread_by_label(project_id, &id)?.is_some() {
                continue;
            }
            let text = match std::fs::read_to_string(entry.path()) {
                Ok(text) => text,
                Err(error) => {
                    errors.push(format!("{name}: {error}"));
                    continue;
                }
            };
            if let Err(error) = crate::thread::ingest_legacy(store, project_id, &text) {
                errors.push(format!("{name}: {error:#}"));
            }
        }
    }
    Ok(errors.join("\n"))
}
