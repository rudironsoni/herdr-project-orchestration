use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use anyhow::Result;

use crate::coordinator::{self, PromptField};
use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::profiles::{self, Change, Entry};
use crate::project::{self, Project, Status};
use crate::runner::RealRunner;
use crate::screen::compose;
use crate::screen::load::Snapshot;
use crate::screen::state::{
    App, Command, FieldKind, Layer, Nav, NewForm, PendingFocus, ThreadForm,
};
use crate::thread::Kind;
use crate::threads::{self, ResolveArgs, StartArgs};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub project_id: String,
    pub slug: String,
    pub target_id: String,
    pub intent: String,
    pub status: String,
    pub message: String,
    pub text: String,
    pub focus_socket: String,
    pub focus_machine: String,
    pub focus_pane: String,
}

impl Outcome {
    fn new(request: &Request, status: &str, message: impl Into<String>) -> Self {
        Self {
            project_id: request.project_id.clone(),
            slug: request.slug.clone(),
            target_id: request.target_id.clone(),
            intent: request.intent.clone(),
            status: status.into(),
            message: message.into(),
            text: request.text.clone(),
            focus_socket: String::new(),
            focus_machine: String::new(),
            focus_pane: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub project_id: String,
    pub slug: String,
    pub intent: String,
    pub command: Command,
    pub text: String,
    pub name: String,
    pub goal: String,
    pub repos: String,
    pub thread_profile: String,
    pub coordinator_profile: String,
    pub title: String,
    pub task: String,
    pub repo: String,
    pub profile: String,
    pub machine: String,
    pub pane: String,
    pub kind: String,
    pub field_value: String,
    pub field_kind: Option<FieldKind>,
    pub target_id: String,
    pub routine: String,
    pub inbox_id: String,
    pub resource: String,
    pub pr: String,
    pub prefix: String,
    pub profile_name: String,
    pub line: Option<usize>,
    pub allocated: bool,
    pub safety_key: String,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            slug: String::new(),
            intent: String::new(),
            command: Command::Nothing,
            text: String::new(),
            name: String::new(),
            goal: String::new(),
            repos: String::new(),
            thread_profile: "claude".into(),
            coordinator_profile: "claude".into(),
            title: String::new(),
            task: String::new(),
            repo: String::new(),
            profile: String::new(),
            machine: String::new(),
            pane: String::new(),
            kind: "worktree".into(),
            field_value: String::new(),
            field_kind: None,
            target_id: String::new(),
            routine: String::new(),
            inbox_id: String::new(),
            resource: String::new(),
            pr: String::new(),
            prefix: String::new(),
            profile_name: String::new(),
            line: None,
            allocated: false,
            safety_key: String::new(),
        }
    }
}

pub struct Lane {
    inflight: Vec<(String, String)>,
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
}

impl Lane {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            inflight: Vec::new(),
            tx,
            rx,
        }
    }

    pub fn schedule<F>(&mut self, project_id: &str, intent: &str, work: F) -> bool
    where
        F: FnOnce() -> Outcome + Send + 'static,
    {
        if intent.is_empty() {
            return false;
        }
        let key = (project_id.to_string(), intent.to_string());
        if self.inflight.iter().any(|item| item == &key) {
            return false;
        }
        self.inflight.push(key);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        true
    }

    pub fn try_recv(&mut self) -> Option<Outcome> {
        match self.rx.try_recv() {
            Ok(outcome) => {
                self.inflight
                    .retain(|item| item.0 != outcome.project_id || item.1 != outcome.intent);
                Some(outcome)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}

pub fn launch(
    lane: &mut Lane,
    ctx: &Ctx,
    app: &mut App,
    snap: &Snapshot,
    command: &Command,
    prefix: Option<&str>,
) {
    let Some(request) = build(app, snap, command, prefix) else {
        return;
    };
    let project_id = request.project_id.clone();
    let intent = request.intent.clone();
    let env = ctx.env.clone();
    let root = ctx.root.clone();
    let config_dir = ctx.config_dir.clone();
    let detached = ctx.detached_ticker;
    let started = lane.schedule(&project_id, &intent, move || {
        let runner = RealRunner;
        let ctx = Ctx {
            env: &env,
            root,
            config_dir,
            runner: &runner,
            detached_ticker: detached,
        };
        perform(&ctx, &request)
    });
    if !started {
        app.notices.insert(project_id, "already running".into());
    }
}

pub fn perform(ctx: &Ctx, request: &Request) -> Outcome {
    let result = match request.command {
        Command::SubmitPrompt => prompt(ctx, request),
        Command::StartCoordinator => open_coordinator(ctx, request),
        Command::FinishSetup => finish(ctx, request),
        Command::SubmitNew => create(ctx, request),
        Command::SubmitThread => start_thread(ctx, request),
        Command::Restart => restart_thread(ctx, request),
        Command::OpenConversation => focus(ctx, request, &request.pane, ""),
        Command::OpenWorker => focus(ctx, request, &request.pane, &request.machine),
        Command::InboxDone => inbox_done(ctx, request),
        Command::ToggleRoutine => toggle_routine(ctx, request),
        Command::Pause => pause(ctx, request),
        Command::Archive => set_life(ctx, request, Status::Archived),
        Command::Delete => delete_project(ctx, request),
        Command::Unarchive => unarchive(ctx, request),
        Command::Rename => rename_project(ctx, request),
        Command::SweepPreview => sweep_preview(ctx, request),
        Command::SweepYes => sweep_yes(ctx, request),
        Command::CopyPath(_) => copy_path(request),
        Command::YoloOn => yolo(ctx, request, "on"),
        Command::YoloOff => yolo(ctx, request, "off"),
        Command::SetSafety { .. } => set_safety(ctx, request),
        Command::Ack => thread_call(ctx, request, threads::ack),
        Command::Stop => thread_call(ctx, request, threads::stop),
        Command::Resolve => resolve_thread(ctx, request),
        Command::NextLine(line) => next_line(ctx, request, line),
        Command::OpenPr => open_pr(ctx, request),
        Command::StartPr => side_prompt(
            ctx,
            request,
            "start a pull request for the selected thread. A recorded reference is not proof that a pull request is open.",
        ),
        Command::Delegate => side_prompt(ctx, request, &task_sentence("delegate", &request.title)),
        Command::CompleteTask => {
            side_prompt(ctx, request, &task_sentence("mark as done", &request.title))
        }
        Command::DropTask => side_prompt(ctx, request, &task_sentence("drop", &request.title)),
        Command::ReadLibrary => read_library(ctx, request),
        Command::OpenResource => open_resource(ctx, request),
        Command::SubmitField => submit_field(ctx, request),
        _ => Ok(Outcome::new(request, "done", "nothing to do")),
    };
    match result {
        Ok(outcome) => outcome,
        Err(error) => Outcome::new(request, "failed", error.to_string()),
    }
}

pub fn apply_outcome(app: &mut App, outcome: &Outcome) {
    match outcome.status.as_str() {
        "confirmed" | "uncertain" => {
            crate::screen::state::apply_prompt_end(
                app,
                &outcome.project_id,
                &outcome.text,
                &outcome.status,
            );
        }
        "confirm" => {
            app.notices
                .insert(outcome.project_id.clone(), outcome.text.clone());
            app.stack.push(Layer::Confirm {
                text: outcome.message.clone(),
                follow: Box::new(Command::SweepYes),
            });
        }
        "refused" => {
            crate::screen::state::apply_prompt_end(
                app,
                &outcome.project_id,
                &outcome.text,
                &format!("refused: {}", outcome.message),
            );
        }
        "allocated" => crate::screen::state::note_thread_allocated(app, &outcome.target_id),
        "focus-later" => {
            app.pending_focus = Some(PendingFocus {
                socket: outcome.focus_socket.clone(),
                machine: outcome.focus_machine.clone(),
                pane: outcome.focus_pane.clone(),
            });
            app.notices
                .insert(outcome.project_id.clone(), outcome.message.clone());
        }
        _ => {
            app.notices
                .insert(outcome.project_id.clone(), outcome.message.clone());
            if outcome.status == "done" && outcome.intent == "open_coordinator" {
                let field = coordinator::prompt_field_after_open_tab();
                let prompt = app.prompt_mut(&outcome.project_id);
                if prompt.field == PromptField::Draft {
                    prompt.field = field;
                }
            }
            if (outcome.status == "done" || outcome.status == "partial")
                && (outcome.intent.starts_with("new:") || outcome.intent == "thread_start")
            {
                app.stack.pop();
            }
            if outcome.status == "failed"
                && outcome.intent.starts_with("new:")
                && let Some(Layer::New(form)) = app.stack.last_mut()
            {
                form.error = outcome.message.clone();
            }
            if outcome.status == "failed"
                && outcome.intent == "thread_start"
                && let Some(Layer::Thread(form)) = app.stack.last_mut()
            {
                form.error = outcome.message.clone();
            }
            if outcome.status == "done"
                && outcome.intent.starts_with("safety")
                && matches!(app.stack.last(), Some(Layer::Field(_)))
            {
                app.stack.pop();
            }
            if outcome.status == "done" && outcome.intent == "rename" {
                app.stack.retain(|layer| !matches!(layer, Layer::Field(_)));
            }
        }
    }
}

pub fn retry_focus(bin: &str, socket: &str, machine: &str, pane: &str) {
    use std::os::unix::process::CommandExt;
    let mut args = Vec::new();
    if !machine.is_empty() {
        args.extend(["--machine".to_string(), machine.to_string()]);
    }
    args.extend(["agent".to_string(), "focus".to_string(), pane.to_string()]);
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args(["-c", "sleep 0.2; exec \"$@\"", "sh", bin])
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

pub(crate) fn build(
    app: &App,
    snap: &Snapshot,
    command: &Command,
    prefix: Option<&str>,
) -> Option<Request> {
    if !runs(command) {
        return None;
    }
    let mut request = Request {
        command: command.clone(),
        prefix: prefix.unwrap_or("").into(),
        ..Request::default()
    };
    if let Some(card) = app.card(snap) {
        request.project_id = card.id.clone();
        request.slug = card.slug.clone();
        let fallback = Nav::default();
        let nav = app.nav.get(&card.id).unwrap_or(&fallback);
        if let Some(thread) = card
            .threads
            .get(compose::clamp(nav.threads.selected, card.threads.len()))
        {
            request.target_id = thread.id.clone();
            request.pane = thread.pane_id.clone();
            request.machine = thread.machine.clone();
            request.pr = thread.pr.clone();
            request.title = thread.title.clone();
        }
        if let Some(task) = card
            .tasks
            .get(compose::clamp(nav.tasks.selected, card.tasks.len()))
            && matches!(
                command,
                Command::Delegate | Command::CompleteTask | Command::DropTask | Command::Notes
            )
        {
            request.title = task.title.clone();
        }
        if let Some(file) = card
            .library
            .get(compose::clamp(nav.library.selected, card.library.len()))
        {
            request.resource = file.label.clone();
        }
        if let Some(pr) = card
            .prs
            .get(compose::clamp(nav.prs.selected, card.prs.len()))
            && matches!(command, Command::OpenPr | Command::StartPr)
        {
            request.pr = pr.reference.clone();
            request.target_id = pr.thread_id.clone();
            request.title = pr.title.clone();
        }
        if let Some(routine) = card
            .routines
            .get(compose::clamp(nav.routines.selected, card.routines.len()))
        {
            request.routine = routine.name.clone();
        }
        if let Some(resource) = card
            .resources
            .get(compose::clamp(nav.resources.selected, card.resources.len()))
            && matches!(command, Command::OpenResource)
        {
            request.resource = resource.label.clone();
        }
        if matches!(command, Command::OpenConversation) {
            request.pane = coordinator_pane(&card.coordinator);
            request.machine.clear();
        }
    }
    if let Some(Layer::New(form)) = app.stack.last() {
        fill_new(&mut request, form);
    }
    if let Some(Layer::Thread(form)) = app.stack.last() {
        fill_thread(&mut request, form);
    }
    if let Some(form) = app.stack.iter().rev().find_map(|layer| match layer {
        Layer::Field(form) => Some(form),
        _ => None,
    }) {
        request.field_kind = Some(form.kind);
        request.field_value = form.value.clone();
        request.profile_name = app.profile_name.clone();
        if form.kind == FieldKind::SafetyArgs || form.kind == FieldKind::Rename {
            request.safety_key = form.title.clone();
        }
    }
    if let Command::CopyPath(path) = command {
        request.resource = path.clone();
    }
    if matches!(command, Command::InboxDone) {
        let index = compose::clamp(app.side_list.selected.saturating_sub(4), snap.inbox.len());
        if let Some(row) = snap.inbox.get(index) {
            request.project_id = row.project_id.clone();
            request.slug = row.slug.clone();
            request.inbox_id = row.id.clone();
        }
    }
    if let Command::NextLine(line) = command {
        request.line = Some(*line);
    }
    if let Command::SetSafety { key, value } = command {
        request.safety_key = key.clone();
        request.field_value = value.clone();
    }
    if matches!(command, Command::SubmitPrompt)
        && let Some(prompt) = app.prompts.get(&request.project_id)
    {
        request.text = prompt.draft.clone();
    }
    if matches!(command, Command::SubmitNew) {
        request.slug = project::slug_from_name(&request.name).unwrap_or_default();
        request.project_id = request.slug.clone();
        if request.prefix.is_empty() {
            return None;
        }
    }
    request.intent = intent(command, &request);
    if request.intent.is_empty() {
        return None;
    }
    Some(request)
}

fn fill_new(request: &mut Request, form: &NewForm) {
    request.name = form.name.clone();
    request.goal = form.goal.clone();
    request.repos = form.repos.clone();
    request.thread_profile = form.thread_profile.clone();
    request.coordinator_profile = form.coordinator_profile.clone();
}

fn fill_thread(request: &mut Request, form: &ThreadForm) {
    request.kind = form.kind_name().into();
    request.title = form.title.clone();
    request.task = form.task.clone();
    request.repo = form.repo.clone();
    request.profile = form.profile.clone();
    request.machine = form.machine.clone();
    request.pane = form.pane.clone();
    request.allocated = form.allocated.is_some();
    if let Some(id) = &form.allocated {
        request.target_id = id.clone();
    }
}

fn intent(command: &Command, request: &Request) -> String {
    match command {
        Command::SubmitPrompt
        | Command::StartPr
        | Command::Delegate
        | Command::CompleteTask
        | Command::DropTask => "prompt".into(),
        Command::StartCoordinator => "open_coordinator".into(),
        Command::FinishSetup => "finish_setup".into(),
        Command::SubmitNew => format!("new:{}", request.slug),
        Command::SubmitThread if !request.allocated => "thread_start".into(),
        Command::SubmitThread | Command::Restart => "thread_restart".into(),
        Command::OpenConversation | Command::OpenWorker => "focus".into(),
        Command::InboxDone => "inbox_done".into(),
        Command::ToggleRoutine => "routine".into(),
        Command::Pause => "pause".into(),
        Command::Archive => "archive".into(),
        Command::Delete => "delete".into(),
        Command::Unarchive => "unarchive".into(),
        Command::Rename => "rename".into(),
        Command::SweepPreview => "sweep_preview".into(),
        Command::SweepYes => "sweep".into(),
        Command::CopyPath(_) => "copy".into(),
        Command::YoloOn | Command::YoloOff => "yolo".into(),
        Command::SetSafety { key, .. } if key == "yolo" => "yolo".into(),
        Command::SetSafety { key, .. } => format!("safety:{key}"),
        Command::SubmitField if request.field_kind == Some(FieldKind::SafetyArgs) => {
            format!("safety:{}", request.safety_key)
        }
        Command::Ack => "ack".into(),
        Command::Stop => "stop".into(),
        Command::Resolve => "resolve".into(),
        Command::NextLine(_) => "next".into(),
        Command::OpenPr => "open_pr".into(),
        Command::ReadLibrary => "read_library".into(),
        Command::OpenResource => "open_resource".into(),
        Command::SubmitField => "field".into(),
        _ => String::new(),
    }
}

fn runs(command: &Command) -> bool {
    !matches!(
        command,
        Command::Nothing
            | Command::Quit
            | Command::FocusNext
            | Command::FocusPrev
            | Command::OpenProjectSelection
            | Command::SelectTab(_)
            | Command::SelectSide(_)
            | Command::SelectProject(_)
            | Command::Move(_)
            | Command::Help
            | Command::Insert(_)
            | Command::Backspace
            | Command::FocusPrompt
            | Command::Cancel
            | Command::ConfirmYes
            | Command::OpenMenu
            | Command::OpenNew
            | Command::OpenSafety
            | Command::SafetyKey(_)
            | Command::Inspect
            | Command::ToggleAdvanced
            | Command::EditGoal
            | Command::EditThreadProfile
            | Command::EditCoordinatorProfile
            | Command::NewProfile
            | Command::RemoveProfile
            | Command::Notes
            | Command::StartThreadForm
            | Command::StartFilter
            | Command::ClearFilter
            | Command::FilterInsert(_)
            | Command::FilterBackspace
            | Command::RenameStart
    )
}

fn prompt(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let delivery = coordinator::prompt_confirmed(ctx, &request.slug, &request.text);
    let field = coordinator::prompt_field(&delivery);
    let outcome = match field {
        PromptField::Confirmed => Outcome::new(request, "confirmed", "confirmed"),
        PromptField::Uncertain => Outcome::new(request, "uncertain", "uncertain"),
        PromptField::Refused => Outcome::new(
            request,
            "refused",
            delivery
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "refused".into()),
        ),
        PromptField::Draft | PromptField::Submitting => Outcome::new(request, "refused", "refused"),
    };
    Ok(outcome)
}

fn open_coordinator(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    coordinator::open(ctx, &request.slug, &coordinator::screen_open_options())?;
    Ok(Outcome::new(
        request,
        "done",
        "coordinator start requested; the draft stays until a prompt is confirmed",
    ))
}

fn finish(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let project = Project::load(&ctx.root, &request.slug)?;
    project::finish_setup(&project, &request.prefix)?;
    Ok(Outcome::new(request, "done", "setup is complete"))
}

fn create(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let repos = request
        .repos
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(project::parse_repo_arg)
        .collect();
    let spec = project::NewProject {
        name: request.name.clone(),
        goal: request.goal.clone(),
        repos,
        thread_profile: request.thread_profile.clone(),
        coordinator_profile: request.coordinator_profile.clone(),
        prefix: request.prefix.clone(),
    };
    match project::create_with_setup(&ctx.root, &spec) {
        Ok(project) => {
            let mut outcome = Outcome::new(request, "done", format!("created {}", project.slug));
            outcome.slug = project.slug;
            Ok(outcome)
        }
        Err(error) => {
            let text = error.to_string();
            let status = if text.contains("was created") {
                "partial"
            } else {
                "failed"
            };
            Ok(Outcome::new(request, status, text))
        }
    }
}

fn allocated_message(form: &threads::StartForm, error: String) -> String {
    let operation = form.operation_id.clone().unwrap_or_default();
    let recorded = form
        .status
        .map(|status| format!("{status:?}"))
        .unwrap_or_default();
    match (operation.is_empty(), recorded.is_empty()) {
        (false, false) => format!("{error} operation {operation} status {recorded}"),
        (false, true) => format!("{error} operation {operation}"),
        (true, false) => format!("{error} status {recorded}"),
        (true, true) => error,
    }
}

fn start_thread(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    if request.allocated {
        let mut outcome = Outcome::new(
            request,
            "failed",
            format!(
                "thread {} was allocated; restart that thread",
                request.target_id
            ),
        );
        outcome.status = "allocated".to_string();
        return Ok(outcome);
    }
    if request.kind == "adopted" {
        let thread = crate::adopt::adopt(
            ctx,
            &request.slug,
            &request.pane,
            &request.title,
            Some(request.task.clone()),
        )?;
        let mut outcome = Outcome::new(request, "done", format!("adopted {}", thread.id));
        outcome.target_id = thread.id;
        return Ok(outcome);
    }
    let kind = match request.kind.as_str() {
        "tab" => Some(Kind::Tab),
        "checkout" => Some(Kind::Checkout),
        _ => Some(Kind::Worktree),
    };
    let repo = (!request.repo.is_empty()).then(|| request.repo.clone());
    let machine = (!request.machine.is_empty()).then(|| request.machine.clone());
    let profile = (!request.profile.is_empty()).then(|| request.profile.clone());
    let form = threads::form_after_start(threads::start(
        ctx,
        &request.slug,
        StartArgs {
            title: request.title.clone(),
            repo,
            machine,
            profile,
            kind,
            base: None,
            task: request.task.clone(),
        },
    ));
    if threads::repeat_start_refused(&form) {
        let id = form.id.clone().unwrap_or_default();
        let mut outcome = if let Some(error) = form.error.clone() {
            Outcome::new(request, "allocated", allocated_message(&form, error))
        } else {
            Outcome::new(request, "done", format!("started {id}"))
        };
        outcome.target_id = id;
        return Ok(outcome);
    }
    Ok(Outcome::new(
        request,
        "failed",
        form.error
            .unwrap_or_else(|| "the thread was not allocated".into()),
    ))
}

fn restart_thread(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let thread = threads::restart(ctx, &request.slug, &request.target_id, None)?;
    let mut outcome = Outcome::new(request, "done", format!("restarted {}", thread.id));
    outcome.target_id = thread.id;
    Ok(outcome)
}

fn focus(ctx: &Ctx, request: &Request, pane: &str, machine: &str) -> Result<Outcome> {
    if pane.is_empty() {
        return Ok(Outcome::new(
            request,
            "failed",
            "no pane; nothing was focused",
        ));
    }
    let project = Project::load(&ctx.root, &request.slug)?;
    let Some(record) = project.coordinator() else {
        return Ok(Outcome::new(request, "failed", "no coordinator record"));
    };
    if record.socket.is_empty() || !Path::new(&record.socket).exists() {
        return Ok(Outcome::new(request, "failed", "stale: socket missing"));
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner).on_machine(machine);
    match herdr.agent_focus(pane) {
        Ok(()) => Ok(Outcome::new(request, "done", format!("focused {pane}"))),
        Err(error) => {
            let mut outcome = Outcome::new(request, "focus-later", error.to_string());
            outcome.focus_socket = record.socket;
            outcome.focus_machine = machine.to_string();
            outcome.focus_pane = pane.to_string();
            Ok(outcome)
        }
    }
}

fn coordinator_pane(line: &str) -> String {
    line.split_whitespace()
        .skip_while(|word| *word != "pane")
        .nth(1)
        .unwrap_or("")
        .to_string()
}

fn inbox_done(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let project = Project::load(&ctx.root, &request.slug)?;
    crate::inbox::done(&project, std::slice::from_ref(&request.inbox_id), false)?;
    Ok(Outcome::new(request, "done", "inbox item marked done"))
}

fn toggle_routine(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    crate::settings::routine_toggle(ctx, &request.slug, &request.routine, None)?;
    Ok(Outcome::new(
        request,
        "done",
        format!("routine {}", request.routine),
    ))
}

fn pause(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let project = Project::load(&ctx.root, &request.slug)?;
    let status = if project.status() == Status::Paused {
        Status::Active
    } else {
        Status::Paused
    };
    crate::lifecycle::set_status(ctx, &request.slug, status)?;
    Ok(Outcome::new(request, "done", format!("lifecycle {status}")))
}

fn set_life(ctx: &Ctx, request: &Request, status: Status) -> Result<Outcome> {
    crate::lifecycle::set_status(ctx, &request.slug, status)?;
    Ok(Outcome::new(request, "done", format!("lifecycle {status}")))
}

fn delete_project(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    crate::lifecycle::delete(ctx, &request.slug, true)?;
    Ok(Outcome::new(request, "done", "deleted"))
}

fn unarchive(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    crate::lifecycle::set_status(ctx, &request.slug, Status::Active)?;
    Ok(Outcome::new(request, "done", "lifecycle active"))
}

fn rename_project(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let to = request.field_value.trim();
    let outcome = crate::rename::run(
        ctx,
        &crate::rename::Args {
            from: &request.slug,
            to,
            name: None,
            dry_run: false,
            by_ticker: false,
        },
    )?;
    Ok(Outcome::new(request, "done", outcome.steps.join("; ")))
}

fn sweep_preview(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let project = Project::load(&ctx.root, &request.slug)?;
    let orphans = crate::sweep::find(ctx, &project);
    if orphans.is_empty() {
        return Ok(Outcome::new(
            request,
            "done",
            format!("{}: nothing to clean", request.slug),
        ));
    }
    let lines: Vec<String> = orphans.iter().map(|orphan| orphan.describe()).collect();
    let mut outcome = Outcome::new(
        request,
        "confirm",
        format!(
            "Remove all {} item(s) listed above from {}? y/N",
            lines.len(),
            request.slug
        ),
    );
    outcome.text = lines.join("\n");
    Ok(outcome)
}

fn sweep_yes(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    crate::sweep::run(ctx, &request.slug, false, true)?;
    Ok(Outcome::new(
        request,
        "done",
        format!("swept {}", request.slug),
    ))
}

fn copy_path(request: &Request) -> Result<Outcome> {
    Ok(Outcome::new(
        request,
        "done",
        crate::popup::copy(&request.resource),
    ))
}

fn yolo(ctx: &Ctx, request: &Request, word: &str) -> Result<Outcome> {
    let target = crate::safety::Target::parse(ctx, &request.slug)?;
    crate::safety::apply(ctx, &target, "yolo", &[word.to_string()])?;
    Ok(Outcome::new(request, "done", format!("yolo {word}")))
}

fn set_safety(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let target = crate::safety::Target::parse(ctx, &request.slug)?;
    crate::safety::apply(
        ctx,
        &target,
        &request.safety_key,
        std::slice::from_ref(&request.field_value),
    )?;
    Ok(Outcome::new(
        request,
        "done",
        format!("{} = {}", request.safety_key, request.field_value),
    ))
}

fn thread_call(
    ctx: &Ctx,
    request: &Request,
    call: impl FnOnce(&Ctx, &str, &str) -> Result<()>,
) -> Result<Outcome> {
    call(ctx, &request.slug, &request.target_id)?;
    Ok(Outcome::new(request, "done", request.target_id.clone()))
}

fn resolve_thread(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    threads::resolve(
        ctx,
        &request.slug,
        &request.target_id,
        &ResolveArgs {
            reopen: false,
            keep_worktree: false,
            skip_copy: false,
            discard_uncopied: false,
        },
    )?;
    Ok(Outcome::new(
        request,
        "done",
        format!("resolved {}", request.target_id),
    ))
}

fn next_line(ctx: &Ctx, request: &Request, line: usize) -> Result<Outcome> {
    threads::next(ctx, &request.slug, &request.target_id, Some(line), None)?;
    Ok(Outcome::new(request, "done", format!("next {line}")))
}

fn open_pr(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    if !crate::pr::valid_pr_url(&request.pr) {
        return Ok(Outcome::new(
            request,
            "failed",
            "recorded reference is not a pull request url",
        ));
    }
    crate::settings::system_open(ctx, &request.pr)?;
    Ok(Outcome::new(
        request,
        "done",
        "opened the recorded reference",
    ))
}

fn side_prompt(ctx: &Ctx, request: &Request, text: &str) -> Result<Outcome> {
    let delivery = coordinator::prompt_confirmed(ctx, &request.slug, text);
    let message = match coordinator::prompt_field(&delivery) {
        PromptField::Confirmed => "confirmed: herdr saw the agent working or blocked",
        PromptField::Uncertain => "uncertain: not sent again",
        PromptField::Refused => {
            let error = delivery
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "refused".into());
            return Ok(Outcome::new(request, "failed", format!("refused: {error}")));
        }
        PromptField::Draft | PromptField::Submitting => {
            return Ok(Outcome::new(request, "failed", "refused"));
        }
    };
    Ok(Outcome::new(request, "noted", message))
}

fn task_sentence(verb: &str, title: &str) -> String {
    format!("(from the projects popup) {verb} the task \"{title}\" in TASKS.md.")
}

fn read_library(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let project = Project::load(&ctx.root, &request.slug)?;
    let path = project.dir().join(&request.resource);
    crate::settings::open_file(ctx, &path, None)?;
    Ok(Outcome::new(request, "done", request.resource.clone()))
}

fn open_resource(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    let path = request.resource.split(" (").next().unwrap_or("");
    if Path::new(path).exists() {
        crate::settings::system_open(ctx, path)?;
        return Ok(Outcome::new(request, "done", path));
    }
    Ok(Outcome::new(request, "done", request.resource.clone()))
}

fn submit_field(ctx: &Ctx, request: &Request) -> Result<Outcome> {
    match request.field_kind {
        Some(FieldKind::Goal) => {
            crate::settings::set(ctx, &request.slug, "goal", &request.field_value)?;
        }
        Some(FieldKind::ThreadProfile) => {
            crate::settings::set(ctx, &request.slug, "thread_profile", &request.field_value)?;
        }
        Some(FieldKind::CoordinatorProfile) => {
            crate::settings::set(
                ctx,
                &request.slug,
                "coordinator_profile",
                &request.field_value,
            )?;
        }
        Some(FieldKind::ProfileAgent) => {
            profiles::require_person()?;
            profiles::apply(
                &ctx.config_dir,
                &Change::Add {
                    name: request.profile_name.clone(),
                    entry: Entry {
                        agent: request.field_value.clone(),
                        ..Entry::default()
                    },
                },
            )?;
        }
        Some(FieldKind::ProfileRemove) => {
            profiles::require_person()?;
            profiles::apply(
                &ctx.config_dir,
                &Change::Remove {
                    name: request.field_value.clone(),
                },
            )?;
        }
        Some(FieldKind::SafetyArgs) => {
            let target = crate::safety::Target::parse(ctx, &request.slug)?;
            crate::safety::apply(
                ctx,
                &target,
                &request.safety_key,
                std::slice::from_ref(&request.field_value),
            )?;
        }
        _ => {
            return Ok(Outcome::new(request, "failed", "the field is incomplete"));
        }
    }
    Ok(Outcome::new(request, "done", "saved"))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crate::coordinator::PromptField;
    use crate::screen::state::App;

    use super::*;

    #[test]
    fn contract_schedule_returns_before_the_job_finishes() {
        let mut lane = Lane::new();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let started = lane.schedule("prj_1", "thread_start", move || {
            let _ = gate_rx.recv();
            Outcome {
                project_id: "prj_1".into(),
                slug: "demo".into(),
                target_id: "t-0001".into(),
                intent: "thread_start".into(),
                status: "done".into(),
                message: "done".into(),
                text: String::new(),
                focus_socket: String::new(),
                focus_machine: String::new(),
                focus_pane: String::new(),
            }
        });
        assert!(started);
        assert!(lane.try_recv().is_none());
        let duplicate = lane.schedule("prj_1", "thread_start", || panic!("duplicate"));
        assert!(!duplicate);
        gate_tx.send(()).unwrap();
        let mut spins = 0;
        let outcome = loop {
            if let Some(outcome) = lane.try_recv() {
                break outcome;
            }
            spins += 1;
            assert!(spins < 10_000, "the job did not finish");
            std::thread::yield_now();
        };
        assert_eq!(outcome.project_id, "prj_1");
        assert_eq!(outcome.target_id, "t-0001");
        assert_eq!(outcome.status, "done");
    }

    #[test]
    fn contract_an_uncertain_outcome_keeps_the_draft() {
        let mut app = App::default();
        app.prompt_mut("prj_1").draft = "look here".into();
        app.prompt_mut("prj_1").field = PromptField::Submitting;
        apply_outcome(
            &mut app,
            &Outcome {
                project_id: "prj_1".into(),
                slug: "demo".into(),
                target_id: String::new(),
                intent: "prompt".into(),
                status: "uncertain".into(),
                message: "uncertain".into(),
                text: "look here".into(),
                focus_socket: String::new(),
                focus_machine: String::new(),
                focus_pane: String::new(),
            },
        );
        let prompt = app.prompts.get("prj_1").unwrap();
        assert_eq!(prompt.field, PromptField::Uncertain);
        assert_eq!(prompt.draft, "look here");
        assert_eq!(prompt.outgoing[0].1, "uncertain: not sent again");
        assert!(!prompt.outgoing[0].1.contains("replied"));
        assert!(!prompt.outgoing[0].1.contains("Accepted"));
    }
}
