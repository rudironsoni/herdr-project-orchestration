//! Managed work. Reservation does not call Herdr. Provision calls it once.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AssignmentId, AttemptId, SettlementId, WorkItemId, WorkerId};
use crate::paths::Ctx;
use crate::project::{self, Project, Repo};
use crate::store::Store;
use crate::thread::{self, Thread};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub work_item_id: String,
    pub worker_id: String,
    pub assignment_id: String,
    pub attempt_id: String,
    pub thread_label: String,
    pub context_hash: String,
    pub payload_hash: String,
    pub brief_hash: Option<String>,
    pub operation_id: String,
}

#[derive(Serialize)]
struct ContextFile {
    name: String,
    hash: String,
}

#[derive(Serialize)]
struct ContextManifest {
    files: Vec<ContextFile>,
}

#[derive(Serialize, Deserialize)]
struct AttemptPayload {
    context: String,
    task: String,
    attempt: String,
    corrections: Vec<String>,
    checkpoint: Option<String>,
}

#[derive(serde::Deserialize)]
struct OperationBody {
    work_item_id: String,
    worker_id: String,
    assignment_id: String,
    attempt_id: String,
    thread_label: String,
    context_hash: String,
    payload_hash: String,
}

pub fn add_item(project: &Project, title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        bail!("the work title is empty");
    }
    let id = WorkItemId::new().to_string();
    let store = Store::open(&project.root)?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    store.insert_work_item(&row.id, &id, title)?;
    Ok(id)
}

pub fn add_worker(project: &Project, label: &str) -> Result<String> {
    let label = label.trim();
    if label.is_empty() {
        bail!("the worker label is empty");
    }
    let id = WorkerId::new().to_string();
    let store = Store::open(&project.root)?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    store.insert_worker(&row.id, &id, label)?;
    Ok(id)
}

pub fn assign(
    project: &Project,
    work_item_id: &str,
    worker_id: &str,
    task: &str,
) -> Result<Reservation> {
    let task = task.trim();
    if task.is_empty() {
        bail!("the task is empty");
    }
    let (settings, _) = project.read_project_md()?;
    let limit = settings.max_parallel_threads;
    let prepared = prepare(project, task)?;
    commit_reservation(
        project,
        work_item_id,
        worker_id,
        limit,
        &prepared.context_hash,
        &prepared.payload_hash,
        &prepared.attempt_id,
    )
}

pub fn resume(project: &Project, operation_id: &str) -> Result<Reservation> {
    let store = Store::open(&project.root)?;
    let operation = store.operation(operation_id)?;
    let body: OperationBody = serde_json::from_str(&operation.payload)
        .with_context(|| format!("operation {operation_id} payload is not a reservation"))?;
    let brief_hash = store.attempt_brief_hash(&body.attempt_id)?;
    Ok(Reservation {
        work_item_id: body.work_item_id,
        worker_id: body.worker_id,
        assignment_id: body.assignment_id,
        attempt_id: body.attempt_id,
        thread_label: body.thread_label,
        context_hash: body.context_hash,
        payload_hash: body.payload_hash,
        brief_hash,
        operation_id: operation.id,
    })
}

pub fn read_blob(project: &Project, hash: &str) -> Result<Vec<u8>> {
    let path = blob_dir(project).join(hash);
    let bytes = std::fs::read(&path).with_context(|| format!("blob {hash} is missing"))?;
    if sha256(&bytes) != hash {
        bail!("blob {hash} is corrupt");
    }
    Ok(bytes)
}

struct Prepared {
    attempt_id: String,
    context_hash: String,
    payload_hash: String,
}

fn prepare(project: &Project, task: &str) -> Result<Prepared> {
    let dir = blob_dir(project);
    let mut files = Vec::new();
    let project_bytes = std::fs::read(project.project_md())?;
    files.push(("PROJECT.md".to_string(), project_bytes));
    let memory = project.dir().join("MEMORY.md");
    if memory.is_file() {
        files.push(("MEMORY.md".to_string(), std::fs::read(&memory)?));
    }
    let memory_dir = project.dir().join("memory");
    if let Ok(entries) = std::fs::read_dir(&memory_dir) {
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".md") && !name.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let path = memory_dir.join(&name);
            let regular = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
            if regular {
                files.push((format!("memory/{name}"), std::fs::read(&path)?));
            }
        }
    }
    let mut manifest_files = Vec::new();
    for (name, bytes) in &files {
        manifest_files.push(ContextFile {
            name: name.clone(),
            hash: publish_blob(&dir, bytes)?,
        });
    }
    let manifest = serde_json::to_vec(&ContextManifest {
        files: manifest_files,
    })?;
    let context_hash = publish_blob(&dir, &manifest)?;
    let attempt_id = AttemptId::new().to_string();
    let payload = serde_json::to_vec(&AttemptPayload {
        context: context_hash.clone(),
        task: task.to_string(),
        attempt: attempt_id.clone(),
        corrections: Vec::new(),
        checkpoint: None,
    })?;
    let payload_hash = publish_blob(&dir, &payload)?;
    Ok(Prepared {
        attempt_id,
        context_hash,
        payload_hash,
    })
}

fn commit_reservation(
    project: &Project,
    work_item_id: &str,
    worker_id: &str,
    limit: u32,
    context_hash: &str,
    payload_hash: &str,
    attempt_id: &str,
) -> Result<Reservation> {
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    let assignment_id = AssignmentId::new().to_string();
    store.immediate(|store| {
        read_blob(project, context_hash)?;
        read_blob(project, payload_hash)?;
        let row = store
            .project_by_slug(&project.slug)?
            .context("project is not in the registry")?;
        if row.lifecycle != "active" {
            bail!("project `{}` is not active", project.slug);
        }
        let (item_project, item_state) = store.work_item_state(work_item_id)?;
        if item_project != row.id {
            bail!("work item {work_item_id} is not in this project");
        }
        if item_state == "accepted" {
            bail!("work item {work_item_id} is accepted");
        }
        let worker_project = store.worker_project(worker_id)?;
        if worker_project != row.id {
            bail!("worker {worker_id} is not in this project");
        }
        let active = store.active_assignment_count(&row.id)?;
        if active >= i64::from(limit) {
            bail!("{active} managed assignments are already active; the managed limit is {limit}");
        }
        let label = next_label(store, &row.id)?;
        let created = project::now();
        let mut record = Thread {
            id: label.clone(),
            title: work_item_id.to_string(),
            created: created.clone(),
            updated: created,
            ..Thread::default()
        };
        record.status = thread::Status::Starting;
        let body = serde_json::to_string(&record)?;
        store.insert_thread_new(&row.id, &label, "provisioning", &body)?;
        store.insert_assignment(
            &assignment_id,
            &row.id,
            work_item_id,
            worker_id,
            &label,
            context_hash,
        )?;
        store.insert_attempt(attempt_id, &row.id, &assignment_id, &label, payload_hash)?;
        let operation_body = serde_json::json!({
            "work_item_id": work_item_id,
            "worker_id": worker_id,
            "assignment_id": assignment_id,
            "attempt_id": attempt_id,
            "thread_label": label,
            "context_hash": context_hash,
            "payload_hash": payload_hash,
        })
        .to_string();
        let key = format!("work_provision:{assignment_id}:{attempt_id}");
        let (operation_id, created) =
            store.begin_operation(&row.id, "work_provision", &operation_body, &key)?;
        if !created {
            bail!("provision {key} already exists");
        }
        Ok(Reservation {
            work_item_id: work_item_id.to_string(),
            worker_id: worker_id.to_string(),
            assignment_id,
            attempt_id: attempt_id.to_string(),
            thread_label: label,
            context_hash: context_hash.to_string(),
            payload_hash: payload_hash.to_string(),
            brief_hash: None,
            operation_id,
        })
    })
}

fn next_label(store: &Store, project_id: &str) -> Result<String> {
    let max = store
        .list_threads(project_id)?
        .iter()
        .filter_map(|row| row.label.strip_prefix("t-")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("t-{:04}", max + 1))
}

fn blob_dir(project: &Project) -> PathBuf {
    project.dir().join(".state").join("blobs")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn publish_blob(dir: &Path, bytes: &[u8]) -> Result<String> {
    let hash = sha256(bytes);
    std::fs::create_dir_all(dir)?;
    let final_path = dir.join(&hash);
    if final_path.is_file() {
        let existing = std::fs::read(&final_path)?;
        if sha256(&existing) != hash {
            bail!("blob {hash} is corrupt");
        }
        return Ok(hash);
    }
    let tmp = dir.join(format!(".partial-{hash}"));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, &final_path)?;
    File::open(dir)?.sync_all()?;
    let written = std::fs::read(&final_path)?;
    if sha256(&written) != hash {
        bail!("blob {hash} changed during publish");
    }
    Ok(hash)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub herdr_workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub cwd: String,
    pub worktree_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRequest {
    pub repo: String,
    pub branch: String,
    pub base: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub thread_label: String,
    pub attempt_id: String,
    pub payload_hash: String,
    pub brief_hash: String,
    pub cwd: String,
}

pub fn provision(
    project: &Project,
    operation_id: &str,
    mut create: impl FnMut(&CheckoutRequest) -> Result<Checkout>,
) -> Result<Delivery> {
    let store = Store::open(&project.root)?;
    let operation = store.operation(operation_id)?;
    let body: OperationBody = serde_json::from_str(&operation.payload)
        .with_context(|| format!("operation {operation_id} payload is not a reservation"))?;
    match operation.step.as_str() {
        "materialized" => load_delivery(project, &body),
        "observed_success" => install_brief(project, operation_id, &body),
        "external_call_started" => {
            mark_recovery(project, &body.assignment_id)?;
            bail!(
                "provision {operation_id} may have created a workspace; reconcile it before another attempt"
            )
        }
        "reserved" => claim_and_create(project, operation_id, &body, &mut create),
        other => bail!("operation {operation_id} is {other}"),
    }
}

pub fn provision_session(ctx: &Ctx, project: &Project, operation_id: &str) -> Result<Delivery> {
    let view = crate::threads::session_view(ctx, project).with_context(|| {
        format!(
            "the herdr session of `{}` is not reachable; run `open {}` first",
            project.slug, project.slug
        )
    })?;
    provision(project, operation_id, |request| {
        let (created, path, cwd) = view
            .herdr
            .worktree_create(
                &request.repo,
                &request.branch,
                &request.base,
                &request.label,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(Checkout {
            herdr_workspace_id: created.workspace_id,
            tab_id: created.tab_id,
            pane_id: created.pane_id,
            cwd,
            worktree_root: path,
        })
    })
}

pub fn reconcile_none(project: &Project, operation_id: &str) -> Result<()> {
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| {
        let operation = store.operation(operation_id)?;
        let body: OperationBody = serde_json::from_str(&operation.payload)?;
        if operation.step != "external_call_started" {
            bail!("operation {operation_id} has not started an external call");
        }
        if store.assignment_state(&body.assignment_id)? != "recovery_required" {
            bail!(
                "assignment {} is not waiting for reconcile",
                body.assignment_id
            );
        }
        store.fail_operation(operation_id, "effect:none")?;
        Ok(())
    })
}

pub fn reconcile_workspace(project: &Project, operation_id: &str, cwd: &str) -> Result<Delivery> {
    if !Path::new(cwd).is_dir() {
        bail!("checkout {cwd} is not a directory");
    }
    let store = Store::open(&project.root)?;
    let operation = store.operation(operation_id)?;
    let body: OperationBody = serde_json::from_str(&operation.payload)?;
    if operation.step != "external_call_started" {
        bail!("operation {operation_id} has not started an external call");
    }
    if store.assignment_state(&body.assignment_id)? != "recovery_required" {
        bail!(
            "assignment {} is not waiting for reconcile",
            body.assignment_id
        );
    }
    record_checkout(
        project,
        operation_id,
        &body,
        &Checkout {
            herdr_workspace_id: String::new(),
            tab_id: String::new(),
            pane_id: String::new(),
            cwd: cwd.to_string(),
            worktree_root: cwd.to_string(),
        },
        &checkout_request(project, &body)?,
    )?;
    install_brief(project, operation_id, &body)
}

pub fn retry(project: &Project, operation_id: &str) -> Result<Reservation> {
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| {
        let operation = store.operation(operation_id)?;
        let error = store.operation_error(operation_id)?;
        if operation.status != "failed" || error != "effect:none" {
            bail!("retry needs reconcile --effect none");
        }
        let body: OperationBody = serde_json::from_str(&operation.payload)?;
        let old: AttemptPayload = serde_json::from_slice(&read_blob(project, &body.payload_hash)?)?;
        let attempt_id = AttemptId::new().to_string();
        let payload = serde_json::to_vec(&AttemptPayload {
            context: old.context,
            task: old.task,
            attempt: attempt_id.clone(),
            corrections: old.corrections,
            checkpoint: old.checkpoint,
        })?;
        let payload_hash = publish_blob(&blob_dir(project), &payload)?;
        let row = store
            .project_by_slug(&project.slug)?
            .context("project is not in the registry")?;
        store.set_attempt_state(&body.attempt_id, "rejected")?;
        store.insert_attempt(
            &attempt_id,
            &row.id,
            &body.assignment_id,
            &body.thread_label,
            &payload_hash,
        )?;
        store.set_assignment_state(&body.assignment_id, "reserved")?;
        let operation_body = serde_json::json!({
            "work_item_id": body.work_item_id,
            "worker_id": body.worker_id,
            "assignment_id": body.assignment_id,
            "attempt_id": attempt_id,
            "thread_label": body.thread_label,
            "context_hash": body.context_hash,
            "payload_hash": payload_hash,
        })
        .to_string();
        let key = format!("work_provision:{}:{attempt_id}", body.assignment_id);
        let (new_operation, created) =
            store.begin_operation(&row.id, "work_provision", &operation_body, &key)?;
        if !created {
            bail!("provision {key} already exists");
        }
        Ok(Reservation {
            work_item_id: body.work_item_id,
            worker_id: body.worker_id,
            assignment_id: body.assignment_id,
            attempt_id,
            thread_label: body.thread_label,
            context_hash: body.context_hash,
            payload_hash,
            brief_hash: None,
            operation_id: new_operation,
        })
    })
}

fn claim_and_create(
    project: &Project,
    operation_id: &str,
    body: &OperationBody,
    create: &mut impl FnMut(&CheckoutRequest) -> Result<Checkout>,
) -> Result<Delivery> {
    read_blob(project, &body.context_hash)?;
    read_blob(project, &body.payload_hash)?;
    let request = checkout_request(project, body)?;
    let claimed = {
        let _lock = project.lock()?;
        let store = Store::open(&project.root)?;
        store.immediate(|store| {
            let operation = store.operation(operation_id)?;
            let row = store
                .project_by_slug(&project.slug)?
                .context("project is not in the registry")?;
            if operation.step != "reserved"
                || operation.status != "pending"
                || row.lifecycle != "active"
                || store.assignment_state(&body.assignment_id)? != "reserved"
            {
                return Ok(false);
            }
            if !store.claim_operation(operation_id, "reserved")? {
                return Ok(false);
            }
            store.set_assignment_state(&body.assignment_id, "provisioning")?;
            Ok(true)
        })?
    };
    if !claimed {
        bail!("provision {operation_id} was not claimed");
    }
    let checkout = match create(&request) {
        Ok(checkout) => checkout,
        Err(error) => {
            mark_recovery(project, &body.assignment_id)?;
            return Err(error);
        }
    };
    record_checkout(project, operation_id, body, &checkout, &request)?;
    install_brief(project, operation_id, body)
}

fn checkout_request(project: &Project, body: &OperationBody) -> Result<CheckoutRequest> {
    let (settings, _) = project.read_project_md()?;
    let repo = local_repo(&settings.repos)?;
    let store = Store::open(&project.root)?;
    let title = store.work_item_title(&body.work_item_id)?;
    Ok(CheckoutRequest {
        repo,
        branch: thread::branch_name(&project.slug, &body.thread_label, &title),
        base: "HEAD".to_string(),
        label: title,
    })
}

fn local_repo(repos: &[Repo]) -> Result<String> {
    repos
        .iter()
        .find(|repo| repo.machine.is_none())
        .map(|repo| repo.path.clone())
        .context("the project has no local repository")
}

fn record_checkout(
    project: &Project,
    operation_id: &str,
    body: &OperationBody,
    checkout: &Checkout,
    request: &CheckoutRequest,
) -> Result<()> {
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| {
        let row = store
            .project_by_slug(&project.slug)?
            .context("project is not in the registry")?;
        let saved = store
            .thread_by_label(&row.id, &body.thread_label)?
            .context("reserved thread disappeared")?;
        let mut record: Thread = serde_json::from_str(&saved.body)?;
        record.repo = request.repo.clone();
        record.branch = request.branch.clone();
        record.base = request.base.clone();
        record.cwd = checkout.cwd.clone();
        record.worktree_path = checkout.worktree_root.clone();
        record.workspace_id = checkout.herdr_workspace_id.clone();
        record.tab_id = checkout.tab_id.clone();
        record.pane_id = checkout.pane_id.clone();
        record.updated = project::now();
        let workspace_id = store.ensure_workspace(
            &row.id,
            &crate::store::WorkspaceDraft {
                kind: "worktree".into(),
                cwd: checkout.cwd.clone(),
                worktree_root: checkout.worktree_root.clone(),
                branch: request.branch.clone(),
                base_ref: request.base.clone(),
                ownership: "managed".into(),
                environment_id: crate::ids::ENV_LOCAL.into(),
                repository_id: None,
            },
        )?;
        store.save_thread(
            &row.id,
            &body.thread_label,
            &saved.lifecycle,
            Some(&workspace_id),
            &serde_json::to_string(&record)?,
        )?;
        store.set_operation_step(operation_id, "observed_success")?;
        Ok(())
    })
}

fn install_brief(project: &Project, operation_id: &str, body: &OperationBody) -> Result<Delivery> {
    let payload_bytes = read_blob(project, &body.payload_hash)?;
    let payload: AttemptPayload = serde_json::from_slice(&payload_bytes)?;
    read_blob(project, &payload.context)?;
    let store = Store::open(&project.root)?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    let saved = store
        .thread_by_label(&row.id, &body.thread_label)?
        .context("reserved thread disappeared")?;
    let record: Thread = serde_json::from_str(&saved.body)?;
    if record.cwd.is_empty() {
        bail!("provision {operation_id} has no checkout path");
    }
    let brief = render_brief(project, &body.thread_label, &record.cwd, &payload)?;
    let brief_hash = publish_blob(&blob_dir(project), &brief)?;
    let dir = PathBuf::from(thread::thread_dir(
        &record.cwd,
        &project.slug,
        &body.thread_label,
    ));
    std::fs::create_dir_all(&dir)?;
    project::write_atomic(&dir.join("brief.md"), &brief)?;
    let written = std::fs::read(dir.join("brief.md"))?;
    if sha256(&written) != brief_hash {
        bail!("brief.md does not match {brief_hash}");
    }
    let task_dir = project.dir().join("threads");
    std::fs::create_dir_all(&task_dir)?;
    project::write_atomic(
        &thread::task_path(project, &body.thread_label),
        payload.task.as_bytes(),
    )?;
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| {
        store.set_attempt_brief(&body.attempt_id, &brief_hash)?;
        store.set_operation_step(operation_id, "materialized")?;
        store.set_assignment_state(&body.assignment_id, "provisioning")?;
        Ok(())
    })?;
    Ok(Delivery {
        thread_label: body.thread_label.clone(),
        attempt_id: body.attempt_id.clone(),
        payload_hash: body.payload_hash.clone(),
        brief_hash,
        cwd: record.cwd,
    })
}

fn load_delivery(project: &Project, body: &OperationBody) -> Result<Delivery> {
    let store = Store::open(&project.root)?;
    let brief_hash = store
        .attempt_brief_hash(&body.attempt_id)?
        .context("materialized provision has no brief")?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    let saved = store
        .thread_by_label(&row.id, &body.thread_label)?
        .context("reserved thread disappeared")?;
    let record: Thread = serde_json::from_str(&saved.body)?;
    Ok(Delivery {
        thread_label: body.thread_label.clone(),
        attempt_id: body.attempt_id.clone(),
        payload_hash: body.payload_hash.clone(),
        brief_hash,
        cwd: record.cwd,
    })
}

pub fn publish_managed_brief(project: &Project, thread: &Thread) -> Result<Option<String>> {
    let store = Store::open(&project.root)?;
    let Some(row) = store.project_by_slug(&project.slug)? else {
        return Ok(None);
    };
    let Some(attempt) = store.open_attempt_for_thread(&row.id, &thread.id)? else {
        return Ok(None);
    };
    if thread.is_remote() {
        bail!("remote managed restart is not available");
    }
    if thread.cwd.is_empty() {
        bail!("managed thread {} has no checkout", thread.id);
    }
    let payload: AttemptPayload =
        serde_json::from_slice(&read_blob(project, &attempt.payload_hash)?)?;
    read_blob(project, &payload.context)?;
    let brief = render_brief(project, &thread.id, &thread.cwd, &payload)?;
    let hash = publish_blob(&blob_dir(project), &brief)?;
    let dir = PathBuf::from(thread::thread_dir(&thread.cwd, &project.slug, &thread.id));
    std::fs::create_dir_all(&dir)?;
    project::write_atomic(&dir.join("brief.md"), &brief)?;
    let written = std::fs::read(dir.join("brief.md"))?;
    if sha256(&written) != hash {
        bail!("brief.md does not match {hash}");
    }
    if attempt.brief_hash.as_deref() != Some(hash.as_str()) {
        let _lock = project.lock()?;
        let store = Store::open(&project.root)?;
        store.set_attempt_brief(&attempt.id, &hash)?;
    }
    Ok(Some(hash))
}

fn render_brief(
    project: &Project,
    thread_label: &str,
    cwd: &str,
    payload: &AttemptPayload,
) -> Result<Vec<u8>> {
    let dir = thread::thread_dir(cwd, &project.slug, thread_label);
    let corrections = if payload.corrections.is_empty() {
        "(none)".to_string()
    } else {
        payload.corrections.join("\n")
    };
    let checkpoint = payload.checkpoint.as_deref().unwrap_or("(none)");
    Ok(format!(
        "# Attempt\n\n- Attempt: {attempt}\n- Context: {context}\n- Task: {task}\n- Corrections: {corrections}\n- Checkpoint: {checkpoint}\n\n# Paths\n\n- Report: `{dir}/report.md`\n- Library: `{dir}/library`\n\n# Evidence\n\nA reviewable result is a clean commit. Do not push. Do not merge.\n",
        attempt = payload.attempt,
        context = payload.context,
        task = payload.task,
    )
    .into_bytes())
}

fn mark_recovery(project: &Project, assignment_id: &str) -> Result<()> {
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| store.set_assignment_state(assignment_id, "recovery_required"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub kind: String,
    pub screen: String,
}

pub fn ingest(project: &Project, report: &str) -> Result<String> {
    let header = parse_report(report)?;
    let store = Store::open(&project.root)?;
    let found = store.attempt(&header.attempt)?;
    if found.state != "reserved" {
        bail!("attempt {} is {}", header.attempt, found.state);
    }
    let work_item_id = store.assignment_work_item(&found.assignment_id)?;
    if header.work != work_item_id || header.assignment != found.assignment_id {
        bail!("report names a different work item or assignment");
    }
    let brief_hash = found
        .brief_hash
        .clone()
        .context("attempt has no delivery brief")?;
    let payload: AttemptPayload =
        serde_json::from_slice(&read_blob(project, &found.payload_hash)?)?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    let saved = store
        .thread_by_label(&row.id, &found.thread_label)?
        .context("reserved thread disappeared")?;
    let record: Thread = serde_json::from_str(&saved.body)?;
    if record.cwd.is_empty() {
        bail!("attempt {} has no checkout", header.attempt);
    }
    let cwd = PathBuf::from(&record.cwd);
    let dirty = dirty_entries(&cwd)?;
    if !dirty.is_empty() {
        bail!("checkout is dirty: {}", dirty[0]);
    }
    if found.evidence_id.is_some() {
        bail!("attempt {} already has evidence", header.attempt);
    }
    let source = git_text(&cwd, &["rev-parse", "HEAD"])?;
    let base = if record.base.is_empty() {
        source.clone()
    } else {
        record.base.clone()
    };
    let package = package_bytes(report, &source, &base, &payload.context, &brief_hash);
    let digest = publish_evidence(project, &package)?;
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    let sealed = store.immediate(|store| store.seal_attempt(&header.attempt, &digest))?;
    if !sealed {
        bail!("attempt {} was already sealed", header.attempt);
    }
    Ok(digest)
}

pub fn settle(project: &Project, attempt_id: &str, reason: &str) -> Result<()> {
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("the settlement reason is empty");
    }
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| {
        let found = store.attempt(attempt_id)?;
        if found.state != "reserved" {
            bail!("attempt {attempt_id} is {}", found.state);
        }
        let evidence = found
            .evidence_id
            .context("attempt has no sealed evidence")?;
        read_evidence(project, &evidence)?;
        store.insert_settlement(&SettlementId::new().to_string(), attempt_id, reason)?;
        store.set_attempt_state(attempt_id, "reported")?;
        store.set_assignment_state(&found.assignment_id, "reported")?;
        Ok(())
    })
}

pub fn reject(
    project: &Project,
    attempt_id: &str,
    evidence: &str,
    reason: &str,
) -> Result<Reservation> {
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("the rejection reason is empty");
    }
    read_evidence(project, evidence)?;
    let store = Store::open(&project.root)?;
    let found = store.attempt(attempt_id)?;
    if found.state != "reported" {
        bail!("attempt {attempt_id} is {}", found.state);
    }
    if found.evidence_id.as_deref() != Some(evidence) {
        bail!("evidence does not match attempt {attempt_id}");
    }
    let assignment_id = found.assignment_id;
    let thread_label = found.thread_label;
    let work_item_id = store.assignment_work_item(&assignment_id)?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    let saved = store
        .thread_by_label(&row.id, &thread_label)?
        .context("reserved thread disappeared")?;
    let record: Thread = serde_json::from_str(&saved.body)?;
    if record.cwd.is_empty() {
        bail!("attempt {attempt_id} has no checkout");
    }
    let old: AttemptPayload = serde_json::from_slice(&read_blob(project, &found.payload_hash)?)?;
    let mut corrections = old.corrections;
    corrections.push(reason.to_string());
    let attempt = AttemptId::new().to_string();
    let payload = serde_json::to_vec(&AttemptPayload {
        context: old.context,
        task: old.task,
        attempt: attempt.clone(),
        corrections,
        checkpoint: old.checkpoint,
    })?;
    let payload_hash = publish_blob(&blob_dir(project), &payload)?;
    let parsed: AttemptPayload = serde_json::from_slice(&payload)?;
    let brief = render_brief(project, &thread_label, &record.cwd, &parsed)?;
    let brief_hash = publish_blob(&blob_dir(project), &brief)?;
    let worker_id = store.assignment_worker(&assignment_id)?;
    let context_hash = store.assignment_context(&assignment_id)?;
    let operation_body = serde_json::json!({
        "work_item_id": &work_item_id,
        "worker_id": &worker_id,
        "assignment_id": &assignment_id,
        "attempt_id": &attempt,
        "thread_label": &thread_label,
        "context_hash": &context_hash,
        "payload_hash": &payload_hash,
    })
    .to_string();
    let key = format!("work_rework:{assignment_id}:{attempt}");
    let created = {
        let _lock = project.lock()?;
        let store = Store::open(&project.root)?;
        store.immediate(|store| {
            let current = store.attempt(attempt_id)?;
            if current.state != "reported" || current.evidence_id.as_deref() != Some(evidence) {
                bail!("attempt {attempt_id} changed before rejection");
            }
            store.set_attempt_state(attempt_id, "rejected")?;
            store.insert_attempt(
                &attempt,
                &row.id,
                &assignment_id,
                &thread_label,
                &payload_hash,
            )?;
            store.set_assignment_state(&assignment_id, "rework_pending")?;
            let (operation_id, inserted) =
                store.begin_operation(&row.id, "work_rework", &operation_body, &key)?;
            if !inserted {
                bail!("rework {key} already exists");
            }
            Ok(Reservation {
                work_item_id,
                worker_id,
                assignment_id,
                attempt_id: attempt,
                thread_label,
                context_hash,
                payload_hash,
                brief_hash: None,
                operation_id,
            })
        })?
    };
    install_rework_brief(project, &created, &brief, &brief_hash)?;
    Ok(Reservation {
        brief_hash: Some(brief_hash),
        ..created
    })
}

pub fn deliver_rework(
    project: &Project,
    operation_id: &str,
    screen: Option<&Screen>,
    mut send: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let store = Store::open(&project.root)?;
    let operation = store.operation(operation_id)?;
    if operation.kind != "work_rework" {
        bail!("operation {operation_id} is not rework");
    }
    let body: OperationBody = serde_json::from_str(&operation.payload)?;
    match operation.step.as_str() {
        "observed_success" => Ok(()),
        "external_call_started" => {
            mark_recovery(project, &body.assignment_id)?;
            bail!("rework {operation_id} may have been sent; do not send it again")
        }
        "materialized" => send_rework(project, operation_id, &body, screen, &mut send),
        other => bail!("rework {operation_id} is {other}"),
    }
}

pub fn deliver_rework_session(ctx: &Ctx, project: &Project, operation_id: &str) -> Result<()> {
    let view = crate::threads::session_view(ctx, project).with_context(|| {
        format!(
            "the herdr session of `{}` is not reachable; run `open {}` first",
            project.slug, project.slug
        )
    })?;
    let store = Store::open(&project.root)?;
    let operation = store.operation(operation_id)?;
    let body: OperationBody = serde_json::from_str(&operation.payload)?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    let saved = store
        .thread_by_label(&row.id, &body.thread_label)?
        .context("reserved thread disappeared")?;
    let record: Thread = serde_json::from_str(&saved.body)?;
    if record.pane_id.is_empty() {
        bail!("thread {} has no pane", body.thread_label);
    }
    let screen = view
        .herdr
        .agent_screen(&record.pane_id)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let kind = if record.agent.is_empty() {
        "claude".to_string()
    } else {
        record.agent.clone()
    };
    let pane = record.pane_id.clone();
    deliver_rework(
        project,
        operation_id,
        Some(&Screen { kind, screen }),
        |text| {
            view.herdr
                .agent_prompt_confirmed(&pane, text)
                .map(|_| ())
                .map_err(|error| anyhow::anyhow!("{error}"))
        },
    )
}

pub fn accept(project: &Project, attempt_id: &str, evidence: &str) -> Result<()> {
    read_evidence(project, evidence)?;
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| {
        let found = store.attempt(attempt_id)?;
        if found.state != "reported" {
            bail!("attempt {attempt_id} is {}", found.state);
        }
        if found.evidence_id.as_deref() != Some(evidence) {
            bail!("evidence does not match attempt {attempt_id}");
        }
        let work_item_id = store.assignment_work_item(&found.assignment_id)?;
        store.set_attempt_state(attempt_id, "accepted")?;
        store.set_assignment_state(&found.assignment_id, "accepted")?;
        store.set_work_item_state(&work_item_id, "accepted")?;
        Ok(())
    })
}

fn send_rework(
    project: &Project,
    operation_id: &str,
    body: &OperationBody,
    screen: Option<&Screen>,
    send: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let screen = screen.context("rework has no pane screen")?;
    if let Some(phrase) = crate::trust_screen::detect(&screen.kind, &screen.screen) {
        bail!(
            "{}",
            crate::trust_screen::refusal(&body.thread_label, "", phrase, true)
        );
    }
    if crate::prompt_box::check(&screen.kind, &screen.screen) == crate::prompt_box::Draft::Typed {
        bail!(
            "draft_in_box: {}'s input box holds typed text; not sending",
            body.thread_label
        );
    }
    let store = Store::open(&project.root)?;
    let found = store.attempt(&body.attempt_id)?;
    let brief_hash = found.brief_hash.context("rework attempt has no brief")?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    let saved = store
        .thread_by_label(&row.id, &body.thread_label)?
        .context("reserved thread disappeared")?;
    let record: Thread = serde_json::from_str(&saved.body)?;
    let path = PathBuf::from(thread::thread_dir(
        &record.cwd,
        &project.slug,
        &body.thread_label,
    ))
    .join("brief.md");
    let on_disk = std::fs::read(&path).with_context(|| format!("missing {}", path.display()))?;
    if sha256(&on_disk) != brief_hash {
        bail!("brief.md does not match {brief_hash}");
    }
    if store.assignment_state(&body.assignment_id)? != "rework_pending" {
        bail!(
            "assignment {} is not waiting for rework",
            body.assignment_id
        );
    }
    let claimed = {
        let _lock = project.lock()?;
        let store = Store::open(&project.root)?;
        store.immediate(|store| {
            let operation = store.operation(operation_id)?;
            if operation.step != "materialized"
                || operation.status != "pending"
                || store.assignment_state(&body.assignment_id)? != "rework_pending"
            {
                return Ok(false);
            }
            store.claim_operation(operation_id, "materialized")
        })?
    };
    if !claimed {
        bail!("rework {operation_id} was not claimed");
    }
    let text = format!(
        "Attempt {}. {}",
        body.attempt_id,
        thread::launch_prompt(&project.slug, &body.thread_label)
    );
    if let Err(error) = send(&text) {
        mark_recovery(project, &body.assignment_id)?;
        return Err(error);
    }
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.set_operation_step(operation_id, "observed_success")?;
    Ok(())
}

fn install_rework_brief(
    project: &Project,
    created: &Reservation,
    brief: &[u8],
    brief_hash: &str,
) -> Result<()> {
    let store = Store::open(&project.root)?;
    let row = store
        .project_by_slug(&project.slug)?
        .context("project is not in the registry")?;
    let saved = store
        .thread_by_label(&row.id, &created.thread_label)?
        .context("reserved thread disappeared")?;
    let record: Thread = serde_json::from_str(&saved.body)?;
    let dir = PathBuf::from(thread::thread_dir(
        &record.cwd,
        &project.slug,
        &created.thread_label,
    ));
    std::fs::create_dir_all(&dir)?;
    project::write_atomic(&dir.join("brief.md"), brief)?;
    let written = std::fs::read(dir.join("brief.md"))?;
    if sha256(&written) != brief_hash {
        bail!("brief.md does not match {brief_hash}");
    }
    let _lock = project.lock()?;
    let store = Store::open(&project.root)?;
    store.immediate(|store| {
        store.set_attempt_brief(&created.attempt_id, brief_hash)?;
        store.set_operation_step(&created.operation_id, "materialized")?;
        Ok(())
    })
}

struct ReportHeader {
    work: String,
    assignment: String,
    attempt: String,
}

fn parse_report(report: &str) -> Result<ReportHeader> {
    let mut work = None;
    let mut assignment = None;
    let mut attempt = None;
    for line in report.lines() {
        if line.is_empty() {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            bail!("report header line `{line}` has no colon");
        };
        let value = value.trim();
        if value.is_empty() {
            bail!("report header `{key}` is empty");
        }
        match key.trim() {
            "work" => work = Some(value.to_string()),
            "assignment" => assignment = Some(value.to_string()),
            "attempt" => attempt = Some(value.to_string()),
            _ => {}
        }
    }
    Ok(ReportHeader {
        work: work.context("report has no work id")?,
        assignment: assignment.context("report has no assignment id")?,
        attempt: attempt.context("report has no attempt id")?,
    })
}

fn package_bytes(report: &str, source: &str, base: &str, context: &str, brief: &str) -> Vec<u8> {
    let mut bytes = report.as_bytes().to_vec();
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    bytes.extend(
        format!("---\nsource: {source}\nbase: {base}\ncontext: {context}\nbrief: {brief}\n")
            .into_bytes(),
    );
    bytes
}

fn evidence_dir(project: &Project) -> PathBuf {
    project.dir().join(".state").join("evidence")
}

fn publish_evidence(project: &Project, bytes: &[u8]) -> Result<String> {
    let hash = sha256(bytes);
    let root = evidence_dir(project);
    std::fs::create_dir_all(&root)?;
    let final_dir = root.join(&hash);
    let package = final_dir.join("package");
    if package.is_file() {
        let existing = std::fs::read(&package)?;
        if sha256(&existing) != hash {
            bail!("evidence {hash} is corrupt");
        }
        return Ok(hash);
    }
    let tmp = root.join(format!(".partial-{hash}"));
    std::fs::create_dir_all(&tmp)?;
    {
        let mut file = File::create(tmp.join("package"))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, &final_dir)?;
    File::open(&root)?.sync_all()?;
    read_evidence(project, &hash)?;
    Ok(hash)
}

fn read_evidence(project: &Project, digest: &str) -> Result<Vec<u8>> {
    let path = evidence_dir(project).join(digest).join("package");
    let bytes = std::fs::read(&path).with_context(|| format!("evidence {digest} is missing"))?;
    if sha256(&bytes) != digest {
        bail!("evidence {digest} is corrupt");
    }
    Ok(bytes)
}

fn git_text(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("git {} failed to start", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn dirty_entries(cwd: &Path) -> Result<Vec<String>> {
    let text = git_text(cwd, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    Ok(text
        .lines()
        .filter(|line| {
            let path = line.get(3..).unwrap_or(line);
            !path.contains(".herdr-project/")
        })
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, Project) {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "ship the change", vec![]).unwrap();
        (root, project)
    }

    fn reserved() -> (tempfile::TempDir, Project, String, String, Reservation) {
        let (root, project) = project();
        let item = add_item(&project, "Fix the gate").unwrap();
        let worker = add_worker(&project, "ada").unwrap();
        let reserved = assign(&project, &item, &worker, "make the check fail closed").unwrap();
        (root, project, item, worker, reserved)
    }

    #[test]
    fn reserve_pins_context_and_leaves_the_delivery_brief_empty() {
        let (_root, project, _item, _worker, reserved) = reserved();
        assert!(reserved.brief_hash.is_none());
        assert!(reserved.attempt_id.starts_with("atm_"));
        assert_eq!(reserved.thread_label, "t-0001");
        let payload = read_blob(&project, &reserved.payload_hash).unwrap();
        let text = String::from_utf8(payload).unwrap();
        assert!(text.contains(&reserved.attempt_id));
        assert!(!text.contains("report_path"));
        assert!(!text.contains("library_path"));
        assert!(
            !project
                .dir()
                .join("threads")
                .join("t-0001.task.md")
                .exists()
        );
        let store = Store::open(&project.root).unwrap();
        assert!(
            store
                .attempt_brief_hash(&reserved.attempt_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn resume_after_commit_does_not_allocate_another_thread() {
        let (_root, project, _item, _worker, reserved) = reserved();
        let again = resume(&project, &reserved.operation_id).unwrap();
        assert_eq!(again.thread_label, reserved.thread_label);
        assert_eq!(again.attempt_id, reserved.attempt_id);
        let store = Store::open(&project.root).unwrap();
        let row = store.project_by_slug(&project.slug).unwrap().unwrap();
        assert_eq!(store.list_threads(&row.id).unwrap().len(), 1);
    }

    #[test]
    fn a_second_assign_of_the_same_item_fails() {
        let (_root, project, item, worker, _reserved) = reserved();
        let err = assign(&project, &item, &worker, "do it again").unwrap_err();
        assert!(err.to_string().contains("UNIQUE"));
        let store = Store::open(&project.root).unwrap();
        let row = store.project_by_slug(&project.slug).unwrap().unwrap();
        assert_eq!(store.list_threads(&row.id).unwrap().len(), 1);
        assert_eq!(store.active_assignment_count(&row.id).unwrap(), 1);
    }

    #[test]
    fn editing_project_md_does_not_change_the_pinned_context() {
        let (_root, project, _item, _worker, reserved) = reserved();
        let before = read_blob(&project, &reserved.context_hash).unwrap();
        std::fs::write(
            project.project_md(),
            "+++\nname = \"Other\"\ngoal = \"nope\"\n+++\n\n",
        )
        .unwrap();
        let after = read_blob(&project, &reserved.context_hash).unwrap();
        assert_eq!(before, after);
        assert!(!String::from_utf8(after).unwrap().contains("nope"));
    }

    #[test]
    fn a_missing_blob_is_not_replaced_with_the_live_file() {
        let (_root, project, _item, _worker, reserved) = reserved();
        let path = blob_dir(&project).join(&reserved.context_hash);
        std::fs::write(&path, b"not the snapshot").unwrap();
        let err = read_blob(&project, &reserved.context_hash).unwrap_err();
        assert!(err.to_string().contains("corrupt"));
    }

    #[test]
    fn a_paused_project_does_not_reserve() {
        let (_root, project) = project();
        let item = add_item(&project, "Fix the gate").unwrap();
        let worker = add_worker(&project, "ada").unwrap();
        let store = Store::open(&project.root).unwrap();
        let row = store.project_by_slug(&project.slug).unwrap().unwrap();
        store.set_lifecycle(&row.id, "paused").unwrap();
        let err = assign(&project, &item, &worker, "make the check fail closed").unwrap_err();
        assert!(err.to_string().contains("not active"));
        assert!(store.list_threads(&row.id).unwrap().is_empty());
    }

    #[test]
    fn a_deleted_blob_rolls_the_reservation_back() {
        let (_root, project) = project();
        let item = add_item(&project, "Fix the gate").unwrap();
        let worker = add_worker(&project, "ada").unwrap();
        let prepared = prepare(&project, "make the check fail closed").unwrap();
        std::fs::remove_file(blob_dir(&project).join(&prepared.context_hash)).unwrap();
        let err = commit_reservation(
            &project,
            &item,
            &worker,
            10,
            &prepared.context_hash,
            &prepared.payload_hash,
            &prepared.attempt_id,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
        let store = Store::open(&project.root).unwrap();
        let row = store.project_by_slug(&project.slug).unwrap().unwrap();
        assert!(store.list_threads(&row.id).unwrap().is_empty());
        assert_eq!(store.active_assignment_count(&row.id).unwrap(), 0);
    }

    #[test]
    fn two_assigns_of_one_item_leave_one_assignment() {
        let (_root, project) = project();
        let item = add_item(&project, "Fix the gate").unwrap();
        let worker = add_worker(&project, "ada").unwrap();
        let project_a = project.clone();
        let project_b = project.clone();
        let item_a = item.clone();
        let item_b = item.clone();
        let worker_a = worker.clone();
        let worker_b = worker;
        let first = std::thread::spawn(move || assign(&project_a, &item_a, &worker_a, "left"));
        let second = std::thread::spawn(move || assign(&project_b, &item_b, &worker_b, "right"));
        let results = [first.join().unwrap(), second.join().unwrap()];
        let wins = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(wins, 1);
        let store = Store::open(&project.root).unwrap();
        let row = store.project_by_slug(&project.slug).unwrap().unwrap();
        assert_eq!(store.active_assignment_count(&row.id).unwrap(), 1);
        assert_eq!(store.list_threads(&row.id).unwrap().len(), 1);
    }

    fn with_repo() -> (tempfile::TempDir, Project, Reservation) {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let project = project::create(
            root.path(),
            "demo",
            "ship the change",
            vec![project::Repo {
                path: repo.to_string_lossy().into_owned(),
                machine: None,
            }],
        )
        .unwrap();
        let item = add_item(&project, "Fix the gate").unwrap();
        let worker = add_worker(&project, "ada").unwrap();
        let reserved = assign(&project, &item, &worker, "make the check fail closed").unwrap();
        (root, project, reserved)
    }

    fn checkout_at(path: &Path) -> Checkout {
        let cwd = path.to_string_lossy().into_owned();
        Checkout {
            herdr_workspace_id: "w1".into(),
            tab_id: "t1".into(),
            pane_id: "p1".into(),
            cwd: cwd.clone(),
            worktree_root: cwd,
        }
    }

    #[test]
    fn provision_writes_the_pinned_brief_after_the_checkout_exists() {
        let (_root, project, reserved) = with_repo();
        assert!(reserved.brief_hash.is_none());
        let text = std::fs::read_to_string(project.project_md()).unwrap();
        std::fs::write(
            project.project_md(),
            text.replace("ship the change", "nope"),
        )
        .unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let mut calls = 0;
        let delivery = provision(&project, &reserved.operation_id, |_| {
            calls += 1;
            Ok(checkout_at(checkout.path()))
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(delivery.payload_hash, reserved.payload_hash);
        assert_ne!(delivery.brief_hash, reserved.context_hash);
        let brief_path = PathBuf::from(thread::thread_dir(
            checkout.path().to_str().unwrap(),
            &project.slug,
            &reserved.thread_label,
        ))
        .join("brief.md");
        let brief = std::fs::read(&brief_path).unwrap();
        assert_eq!(sha256(&brief), delivery.brief_hash);
        let text = String::from_utf8(brief).unwrap();
        assert!(text.contains(&reserved.attempt_id));
        assert!(text.contains(checkout.path().to_str().unwrap()));
        assert!(!text.contains("nope"));
        let again = provision(&project, &reserved.operation_id, |_| {
            calls += 1;
            Ok(checkout_at(checkout.path()))
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(again.brief_hash, delivery.brief_hash);
    }

    #[test]
    fn a_missing_context_blob_does_not_create_a_checkout() {
        let (_root, project, reserved) = with_repo();
        std::fs::remove_file(blob_dir(&project).join(&reserved.context_hash)).unwrap();
        let mut calls = 0;
        let err = provision(&project, &reserved.operation_id, |_| {
            calls += 1;
            Ok(checkout_at(Path::new("/unused")))
        })
        .unwrap_err();
        assert_eq!(calls, 0);
        assert!(err.to_string().contains("missing"));
        let store = Store::open(&project.root).unwrap();
        assert_eq!(
            store.operation(&reserved.operation_id).unwrap().step,
            "reserved"
        );
    }

    #[test]
    fn a_lost_create_is_not_retried_without_reconcile() {
        let (_root, project, reserved) = with_repo();
        let mut calls = 0;
        let err = provision(&project, &reserved.operation_id, |_| {
            calls += 1;
            bail!("reply lost")
        })
        .unwrap_err();
        assert!(err.to_string().contains("reply lost"));
        assert_eq!(calls, 1);
        let err = provision(&project, &reserved.operation_id, |_| {
            calls += 1;
            Ok(checkout_at(Path::new("/unused")))
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(err.to_string().contains("reconcile"));
        let err = retry(&project, &reserved.operation_id).unwrap_err();
        assert!(err.to_string().contains("reconcile"));
        assert_eq!(calls, 1);
        let store = Store::open(&project.root).unwrap();
        assert_eq!(
            store.assignment_state(&reserved.assignment_id).unwrap(),
            "recovery_required"
        );
    }

    #[test]
    fn reconcile_workspace_installs_the_brief_without_another_create() {
        let (_root, project, reserved) = with_repo();
        let mut calls = 0;
        provision(&project, &reserved.operation_id, |_| {
            calls += 1;
            bail!("reply lost")
        })
        .unwrap_err();
        let checkout = tempfile::tempdir().unwrap();
        let delivery = reconcile_workspace(
            &project,
            &reserved.operation_id,
            checkout.path().to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(delivery.payload_hash, reserved.payload_hash);
        let brief = std::fs::read(
            PathBuf::from(thread::thread_dir(
                checkout.path().to_str().unwrap(),
                &project.slug,
                &reserved.thread_label,
            ))
            .join("brief.md"),
        )
        .unwrap();
        assert_eq!(sha256(&brief), delivery.brief_hash);
        assert!(
            !String::from_utf8(brief)
                .unwrap()
                .contains("ship the change")
        );
    }

    #[test]
    fn two_provisions_create_one_checkout() {
        let (_root, project, reserved) = with_repo();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let checkout = tempfile::tempdir().unwrap();
        let path = checkout.path().to_path_buf();
        let first = {
            let project = project.clone();
            let operation = reserved.operation_id.clone();
            let calls = calls.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                provision(&project, &operation, |_| {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(checkout_at(&path))
                })
            })
        };
        let second = {
            let operation = reserved.operation_id;
            let calls = calls.clone();
            std::thread::spawn(move || {
                provision(&project, &operation, |_| {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(checkout_at(&path))
                })
            })
        };
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(results.iter().any(|result| result.is_ok()));
    }

    fn git_commit(dir: &Path) {
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("file.txt"), "one\n").unwrap();
        git(&["add", "file.txt"]);
        git(&["commit", "-m", "init"]);
    }

    fn report_text(reserved: &Reservation, attempt: &str, body: &str) -> String {
        format!(
            "work: {}\nassignment: {}\nattempt: {attempt}\n\n{body}\n",
            reserved.work_item_id, reserved.assignment_id
        )
    }

    fn provisioned() -> (tempfile::TempDir, Project, Reservation, tempfile::TempDir) {
        let (root, project, reserved) = with_repo();
        let checkout = tempfile::tempdir().unwrap();
        git_commit(checkout.path());
        provision(&project, &reserved.operation_id, |_| {
            Ok(checkout_at(checkout.path()))
        })
        .unwrap();
        (root, project, reserved, checkout)
    }

    #[test]
    fn a_clean_report_seals_and_a_later_report_does_not_change_it() {
        let (_root, project, reserved, checkout) = provisioned();
        let digest = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "done"),
        )
        .unwrap();
        let package = evidence_dir(&project).join(&digest).join("package");
        let first = std::fs::read(&package).unwrap();
        std::fs::write(checkout.path().join("extra.txt"), "x\n").unwrap();
        let dirty = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "later"),
        )
        .unwrap_err();
        assert!(dirty.to_string().contains("dirty"));
        std::fs::remove_file(checkout.path().join("extra.txt")).unwrap();
        let again = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "later"),
        )
        .unwrap_err();
        assert!(again.to_string().contains("already"));
        assert_eq!(std::fs::read(&package).unwrap(), first);
        settle(&project, &reserved.attempt_id, "the turn ended").unwrap();
        let store = Store::open(&project.root).unwrap();
        let found = store.attempt(&reserved.attempt_id).unwrap();
        assert_eq!(found.state, "reported");
    }

    #[test]
    fn reject_installs_the_next_brief_and_keeps_the_old_report_out() {
        let (_root, project, reserved, checkout) = provisioned();
        let digest = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "done"),
        )
        .unwrap();
        settle(&project, &reserved.attempt_id, "the turn ended").unwrap();
        let mut calls = 0;
        let next = reject(&project, &reserved.attempt_id, &digest, "fix the name").unwrap();
        let brief = std::fs::read(
            PathBuf::from(thread::thread_dir(
                checkout.path().to_str().unwrap(),
                &project.slug,
                &reserved.thread_label,
            ))
            .join("brief.md"),
        )
        .unwrap();
        let text = String::from_utf8(brief).unwrap();
        assert!(text.contains(&next.attempt_id));
        assert!(text.contains("fix the name"));
        assert!(!text.contains(&format!("Attempt: {}", reserved.attempt_id)));
        let old = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "done"),
        )
        .unwrap_err();
        assert!(old.to_string().contains("rejected"));
        let screen = Screen {
            kind: "claude".into(),
            screen: "Do you trust the files in this folder?".into(),
        };
        let blocked = deliver_rework(&project, &next.operation_id, Some(&screen), |_| {
            calls += 1;
            Ok(())
        })
        .unwrap_err();
        assert!(blocked.to_string().contains("trust"));
        assert_eq!(calls, 0);
        let rule = "─".repeat(40);
        let typed = Screen {
            kind: "claude".into(),
            screen: format!("{rule}\n❯ \n  second line typed\n{rule}\n"),
        };
        let blocked = deliver_rework(&project, &next.operation_id, Some(&typed), |_| {
            calls += 1;
            Ok(())
        })
        .unwrap_err();
        assert!(blocked.to_string().contains("draft_in_box"));
        assert_eq!(calls, 0);
        deliver_rework(
            &project,
            &next.operation_id,
            Some(&Screen {
                kind: "unknown".into(),
                screen: "idle\n".into(),
            }),
            |_| {
                calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        deliver_rework(
            &project,
            &next.operation_id,
            Some(&Screen {
                kind: "unknown".into(),
                screen: "idle\n".into(),
            }),
            |_| {
                calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        let err = accept(&project, &reserved.attempt_id, &digest).unwrap_err();
        assert!(err.to_string().contains("rejected"));
    }

    #[test]
    fn a_lost_rework_send_is_not_repeated() {
        let (_root, project, reserved, _checkout) = provisioned();
        let digest = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "done"),
        )
        .unwrap();
        settle(&project, &reserved.attempt_id, "the turn ended").unwrap();
        let next = reject(&project, &reserved.attempt_id, &digest, "fix the name").unwrap();
        let mut calls = 0;
        let screen = Screen {
            kind: "unknown".into(),
            screen: "idle\n".into(),
        };
        deliver_rework(&project, &next.operation_id, Some(&screen), |_| {
            calls += 1;
            bail!("reply lost")
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        deliver_rework(&project, &next.operation_id, Some(&screen), |_| {
            calls += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(calls, 1);
    }

    #[test]
    fn accept_names_the_sealed_digest_and_drops_the_active_slot() {
        let (_root, project, reserved, _checkout) = provisioned();
        let digest = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "done"),
        )
        .unwrap();
        settle(&project, &reserved.attempt_id, "the turn ended").unwrap();
        let err = accept(&project, &reserved.attempt_id, "not-the-digest").unwrap_err();
        assert!(err.to_string().contains("missing") || err.to_string().contains("corrupt"));
        accept(&project, &reserved.attempt_id, &digest).unwrap();
        let store = Store::open(&project.root).unwrap();
        let row = store.project_by_slug(&project.slug).unwrap().unwrap();
        assert_eq!(store.active_assignment_count(&row.id).unwrap(), 0);
        assert_eq!(store.list_threads(&row.id).unwrap().len(), 1);
        let found = store.attempt(&reserved.attempt_id).unwrap();
        assert_eq!(found.state, "accepted");
    }

    #[test]
    fn restart_republishes_the_current_attempt_without_a_new_payload() {
        let (_root, project, reserved, checkout) = provisioned();
        let digest = ingest(
            &project,
            &report_text(&reserved, &reserved.attempt_id, "done"),
        )
        .unwrap();
        settle(&project, &reserved.attempt_id, "the turn ended").unwrap();
        let next = reject(&project, &reserved.attempt_id, &digest, "fix the name").unwrap();
        let mut calls = 0;
        deliver_rework(
            &project,
            &next.operation_id,
            Some(&Screen {
                kind: "unknown".into(),
                screen: "idle\n".into(),
            }),
            |_| {
                calls += 1;
                Ok(())
            },
        )
        .unwrap();
        let brief_path = PathBuf::from(thread::thread_dir(
            checkout.path().to_str().unwrap(),
            &project.slug,
            &reserved.thread_label,
        ))
        .join("brief.md");
        let original = std::fs::read(&brief_path).unwrap();
        let original_hash = sha256(&original);
        assert_eq!(next.brief_hash.as_deref(), Some(original_hash.as_str()));
        let project_md = std::fs::read_to_string(project.project_md()).unwrap();
        std::fs::write(
            project.project_md(),
            project_md.replace("ship the change", "nope"),
        )
        .unwrap();
        let thread = thread::load(&project, &reserved.thread_label).unwrap();
        let same = publish_managed_brief(&project, &thread).unwrap().unwrap();
        assert_eq!(same, original_hash);
        assert_eq!(std::fs::read(&brief_path).unwrap(), original);
        let store = Store::open(&project.root).unwrap();
        assert_eq!(
            store.operation(&next.operation_id).unwrap().step,
            "observed_success"
        );
        assert_eq!(calls, 1);
        let other = tempfile::tempdir().unwrap();
        let cwd = other.path().to_string_lossy().into_owned();
        thread::update(&project, &reserved.thread_label, |record| {
            record.cwd = cwd.clone();
            record.worktree_path = cwd;
        })
        .unwrap();
        let moved = thread::load(&project, &reserved.thread_label).unwrap();
        let new_hash = publish_managed_brief(&project, &moved).unwrap().unwrap();
        assert_ne!(new_hash, original_hash);
        let brief = std::fs::read(
            PathBuf::from(thread::thread_dir(
                other.path().to_str().unwrap(),
                &project.slug,
                &reserved.thread_label,
            ))
            .join("brief.md"),
        )
        .unwrap();
        assert_eq!(sha256(&brief), new_hash);
        let text = String::from_utf8(brief).unwrap();
        assert!(text.contains(other.path().to_str().unwrap()));
        assert!(text.contains(&next.attempt_id));
        assert!(text.contains("fix the name"));
        assert!(!text.contains("nope"));
        let found = store.attempt(&next.attempt_id).unwrap();
        assert_eq!(found.payload_hash, next.payload_hash);
        assert_eq!(found.id, next.attempt_id);
        assert_eq!(found.assignment_id, reserved.assignment_id);
        let work_item = store.assignment_work_item(&found.assignment_id).unwrap();
        assert_eq!(work_item, reserved.work_item_id);
    }
}
