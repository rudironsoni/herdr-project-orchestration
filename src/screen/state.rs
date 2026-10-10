use std::collections::BTreeMap;

use crate::coordinator::{self, PromptField};
use crate::screen::load::{Card, Snapshot};

pub const TABS: [&str; 6] = [
    "Threads",
    "Tasks",
    "Library",
    "PRs",
    "Routines",
    "Resources",
];

pub const MENU: [&str; 13] = [
    "Edit goal",
    "Thread profile",
    "Coordinator profile",
    "New profile",
    "Delete profile",
    "Yolo on",
    "Yolo off",
    "Pause or resume",
    "Archive",
    "Delete",
    "Safety",
    "Unarchive",
    "Rename",
];

pub fn menu_len() -> usize {
    MENU.len()
}

pub fn menu_command(index: usize) -> Command {
    match index {
        0 => Command::EditGoal,
        1 => Command::EditThreadProfile,
        2 => Command::EditCoordinatorProfile,
        3 => Command::NewProfile,
        4 => Command::RemoveProfile,
        5 => Command::YoloOn,
        6 => Command::YoloOff,
        7 => Command::Pause,
        8 => Command::Archive,
        9 => Command::Delete,
        10 => Command::OpenSafety,
        11 => Command::Unarchive,
        12 => Command::RenameStart,
        _ => Command::Nothing,
    }
}

pub fn project_visible(name: &str, slug: &str, filter: Option<&str>) -> bool {
    let Some(raw) = filter else {
        return true;
    };
    if raw.is_empty() {
        return true;
    }
    let needle = raw.to_lowercase();
    name.to_lowercase().contains(&needle) || slug.to_lowercase().contains(&needle)
}

pub fn side_len(snap: &Snapshot, side: Side, filter: Option<&str>) -> usize {
    let head = 4 + usize::from(filter.is_some());
    match side {
        Side::All => {
            head + snap
                .projects
                .iter()
                .filter(|card| project_visible(&card.name, &card.slug, filter))
                .count()
                + snap.failed.len()
                + 4
        }
        Side::Needs => head + snap.needs.len().max(1),
        Side::Inbox => head + snap.inbox.len().max(1),
    }
}

pub fn detail_count(card: Option<&Card>, nav: &Nav) -> usize {
    let files = if !nav.overview && nav.tab == 0 {
        card.and_then(|card| {
            crate::screen::compose::thread_by_selection(card, nav.threads.selected)
        })
        .map(|thread| thread.files.len())
        .unwrap_or(0)
    } else {
        0
    };
    detail_len() + files
}

pub fn detail_len() -> usize {
    4
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Global,
    Projects,
    Work,
    Tabs,
    Overview,
    Prompt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    All,
    Needs,
    Inbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Columns {
    Three,
    Stacked,
    TooSmall,
}

pub fn columns(width: u16) -> Columns {
    if width >= 100 {
        Columns::Three
    } else if width >= 24 {
        Columns::Stacked
    } else {
        Columns::TooSmall
    }
}

pub fn cockpit_fits(width: u16, height: u16) -> bool {
    width >= 24 && height >= 10
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListPos {
    pub selected: usize,
    pub offset: usize,
}

impl ListPos {
    pub fn set(&mut self, index: usize, len: usize, window: usize) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        self.selected = index.min(len - 1);
        let window = window.max(1);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + window {
            self.offset = self.selected + 1 - window;
        }
    }

    pub fn move_by(&mut self, delta: i32, len: usize, window: usize) {
        let next = if delta < 0 {
            self.selected.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.selected.saturating_add(delta as usize)
        };
        self.set(next, len, window);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nav {
    pub tab: usize,
    pub overview: bool,
    pub threads: ListPos,
    pub tasks: ListPos,
    pub library: ListPos,
    pub prs: ListPos,
    pub routines: ListPos,
    pub resources: ListPos,
    pub work: ListPos,
    pub detail: ListPos,
    pub overview_pos: ListPos,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            tab: 0,
            overview: true,
            threads: ListPos::default(),
            tasks: ListPos::default(),
            library: ListPos::default(),
            prs: ListPos::default(),
            routines: ListPos::default(),
            resources: ListPos::default(),
            work: ListPos::default(),
            detail: ListPos::default(),
            overview_pos: ListPos::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptState {
    pub draft: String,
    pub field: PromptField,
    pub outgoing: Vec<(String, String)>,
}

impl Default for PromptState {
    fn default() -> Self {
        Self {
            draft: String::new(),
            field: PromptField::Draft,
            outgoing: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewForm {
    pub name: String,
    pub goal: String,
    pub repos: String,
    pub thread_profile: String,
    pub coordinator_profile: String,
    pub advanced: bool,
    pub field: usize,
    pub error: String,
}

impl Default for NewForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            goal: String::new(),
            repos: String::new(),
            thread_profile: "claude".into(),
            coordinator_profile: "claude".into(),
            advanced: false,
            field: 0,
            error: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadForm {
    pub kind: usize,
    pub title: String,
    pub task: String,
    pub repo: String,
    pub profile: String,
    pub machine: String,
    pub pane: String,
    pub field: usize,
    pub allocated: Option<String>,
    pub error: String,
}

impl ThreadForm {
    pub fn kind_name(&self) -> &'static str {
        ["worktree", "tab", "checkout", "adopted"][self.kind.min(3)]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldForm {
    pub title: String,
    pub value: String,
    pub kind: FieldKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Goal,
    ThreadProfile,
    CoordinatorProfile,
    ProfileName,
    ProfileAgent,
    ProfileRemove,
    SafetyArgs,
    Rename,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafetyPick {
    pub key: String,
    pub options: Vec<String>,
    pub selected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layer {
    Picker,
    Help,
    New(NewForm),
    Thread(ThreadForm),
    Field(FieldForm),
    Detail,
    Menu,
    Safety,
    SafetyPick(SafetyPick),
    Confirm { text: String, follow: Box<Command> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Nothing,
    Quit,
    FocusNext,
    FocusPrev,
    OpenProjectSelection,
    ShowOverview,
    SelectTab(usize),
    SelectSide(Side),
    SelectProject(usize),
    Move(i32),
    OpenWorker,
    Help,
    Insert(char),
    Backspace,
    FocusPrompt,
    SubmitPrompt,
    FinishSetup,
    StartCoordinator,
    OpenConversation,
    StartThreadForm,
    SubmitThread,
    SubmitNew,
    SubmitField,
    Cancel,
    ConfirmYes,
    InboxDone(String),
    ToggleRoutine,
    OpenMenu,
    Pause,
    Archive,
    Delete,
    YoloOn,
    YoloOff,
    Ack,
    Stop,
    Restart,
    Resolve,
    NextLine(usize),
    OpenPr,
    StartPr,
    Notes,
    Delegate,
    CompleteTask,
    DropTask,
    ReadLibrary,
    OpenResource,
    NewProfile,
    RemoveProfile,
    EditGoal,
    EditThreadProfile,
    EditCoordinatorProfile,
    ToggleAdvanced,
    OpenNew,
    Inspect,
    OpenSafety,
    SafetyKey(String),
    SetSafety { key: String, value: String },
    StartFilter,
    ClearFilter,
    FilterInsert(char),
    FilterBackspace,
    SweepPreview,
    SweepYes,
    Unarchive,
    RenameStart,
    Rename,
    CopyPath(String),
    InspectCoordinator,
    OpenAttention { project: usize, thread_id: String },
}

#[derive(Clone, Debug)]
pub struct App {
    pub focus: Focus,
    pub one: Focus,
    pub stack: Vec<Layer>,
    pub g: bool,
    pub side: Side,
    pub side_list: ListPos,
    pub menu_list: ListPos,
    pub safety_list: ListPos,
    pub picker_list: ListPos,
    pub project_ix: usize,
    pub nav: BTreeMap<String, Nav>,
    pub prompts: BTreeMap<String, PromptState>,
    pub notices: BTreeMap<String, String>,
    pub profile_name: String,
    pub profile_agent: String,
    pub pending_focus: Option<PendingFocus>,
    pub filter: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingFocus {
    pub socket: String,
    pub machine: String,
    pub pane: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            focus: Focus::Work,
            one: Focus::Work,
            stack: Vec::new(),
            g: false,
            side: Side::All,
            side_list: ListPos::default(),
            menu_list: ListPos::default(),
            safety_list: ListPos::default(),
            picker_list: ListPos::default(),
            project_ix: 0,
            nav: BTreeMap::new(),
            prompts: BTreeMap::new(),
            notices: BTreeMap::new(),
            profile_name: String::new(),
            profile_agent: "claude".into(),
            pending_focus: None,
            filter: None,
        }
    }
}

impl App {
    pub fn select_slug(&mut self, snap: &Snapshot, slug: &str) {
        if let Some(index) = snap.projects.iter().position(|card| card.slug == slug) {
            self.project_ix = index;
        }
    }

    pub fn card<'a>(&self, snap: &'a Snapshot) -> Option<&'a Card> {
        snap.projects.get(self.project_ix)
    }

    pub fn project_id(&self, snap: &Snapshot) -> Option<String> {
        self.card(snap).map(|card| card.id.clone())
    }

    pub fn nav_mut(&mut self, id: &str) -> &mut Nav {
        self.nav.entry(id.to_string()).or_default()
    }

    pub fn prompt_mut(&mut self, id: &str) -> &mut PromptState {
        self.prompts.entry(id.to_string()).or_default()
    }
}

pub fn apply(app: &mut App, command: Command, snap: &Snapshot, width: u16) -> Command {
    let cols = columns(width);
    match command {
        Command::Nothing | Command::Quit => command,
        Command::FocusNext => {
            app.focus = next_focus(app.focus, cols, true);
            if matches!(app.focus, Focus::Work | Focus::Overview) {
                app.one = app.focus;
            }
            Command::Nothing
        }
        Command::FocusPrev => {
            app.focus = next_focus(app.focus, cols, false);
            if matches!(app.focus, Focus::Work | Focus::Overview) {
                app.one = app.focus;
            }
            Command::Nothing
        }
        Command::FocusPrompt => {
            app.focus = Focus::Prompt;
            Command::Nothing
        }
        Command::OpenProjectSelection => {
            app.g = false;
            if cockpit_fits(width, 10) && cols != Columns::TooSmall {
                app.focus = Focus::Projects;
            } else {
                app.stack.push(Layer::Picker);
            }
            Command::Nothing
        }
        Command::ShowOverview => {
            if let Some(id) = app.project_id(snap) {
                app.nav_mut(&id).overview = true;
            }
            app.focus = Focus::Overview;
            app.one = Focus::Overview;
            Command::Nothing
        }
        Command::SelectTab(index) => {
            app.g = false;
            if let Some(id) = app.project_id(snap) {
                let nav = app.nav_mut(&id);
                nav.overview = false;
                nav.tab = index.min(5);
            }
            app.focus = Focus::Overview;
            app.one = Focus::Overview;
            Command::Nothing
        }
        Command::SelectSide(side) => {
            app.side = side;
            app.focus = Focus::Projects;
            Command::Nothing
        }
        Command::SelectProject(index) => {
            if index < snap.projects.len() {
                app.project_ix = index;
                app.side = Side::All;
            }
            if matches!(app.stack.last(), Some(Layer::Picker)) {
                app.stack.pop();
            }
            Command::Nothing
        }
        Command::Move(delta) => {
            move_list(app, snap, delta);
            Command::Nothing
        }
        Command::Cancel => {
            app.g = false;
            app.filter = None;
            if app.stack.pop().is_none() {
                if app.focus == Focus::Prompt {
                    app.focus = Focus::Work;
                    app.one = Focus::Work;
                    Command::Nothing
                } else {
                    Command::Quit
                }
            } else {
                Command::Nothing
            }
        }
        Command::StartFilter => {
            app.g = false;
            app.filter = Some(String::new());
            app.picker_list = ListPos::default();
            app.side_list = ListPos::default();
            app.side = Side::All;
            if cols != Columns::TooSmall {
                app.focus = Focus::Projects;
                if !matches!(
                    app.stack.last(),
                    Some(Layer::New(_) | Layer::Thread(_) | Layer::Field(_))
                ) {
                    app.stack.clear();
                }
            } else if !matches!(app.stack.last(), Some(Layer::Picker))
                && !matches!(
                    app.stack.last(),
                    Some(Layer::New(_) | Layer::Thread(_) | Layer::Field(_))
                )
            {
                app.stack.retain(|layer| {
                    matches!(layer, Layer::New(_) | Layer::Thread(_) | Layer::Field(_))
                });
                app.stack.push(Layer::Picker);
            }
            Command::Nothing
        }
        Command::ClearFilter => {
            app.filter = None;
            app.picker_list = ListPos::default();
            app.side_list = ListPos::default();
            Command::Nothing
        }
        Command::FilterInsert(ch) => {
            if let Some(filter) = &mut app.filter {
                filter.push(ch);
            }
            app.picker_list = ListPos::default();
            app.side_list = ListPos::default();
            Command::Nothing
        }
        Command::FilterBackspace => {
            if let Some(filter) = &mut app.filter {
                filter.pop();
            }
            app.picker_list = ListPos::default();
            app.side_list = ListPos::default();
            Command::Nothing
        }
        Command::Help => {
            app.stack.push(Layer::Help);
            Command::Nothing
        }
        Command::Insert(ch) => {
            insert_char(app, snap, ch);
            Command::Nothing
        }
        Command::Backspace => {
            backspace(app, snap);
            Command::Nothing
        }
        Command::SubmitPrompt => submit_prompt(app, snap),
        Command::OpenMenu => {
            app.stack.push(Layer::Menu);
            Command::Nothing
        }
        Command::StartThreadForm => {
            app.stack.push(Layer::Thread(ThreadForm::default()));
            Command::Nothing
        }
        Command::SubmitNew => submit_new(app),
        Command::OpenNew => {
            app.stack.push(Layer::New(NewForm::default()));
            Command::Nothing
        }
        Command::Inspect => {
            if let Some(id) = app.project_id(snap) {
                app.nav_mut(&id).detail = ListPos::default();
            }
            app.stack.push(Layer::Detail);
            Command::Nothing
        }
        Command::InspectCoordinator => {
            if let Some(id) = app.project_id(snap) {
                let line = app
                    .card(snap)
                    .map(|card| card.coordinator.clone())
                    .unwrap_or_else(|| "coordinator missing".into());
                app.notices.insert(id, format!("not focused; {line}"));
            }
            Command::Nothing
        }
        Command::OpenAttention { project, thread_id } => {
            open_attention(app, snap, project, &thread_id);
            Command::Nothing
        }
        Command::SubmitThread => submit_thread(app),
        Command::SubmitField => submit_field_step(app),
        Command::ToggleAdvanced => {
            if let Some(Layer::New(form)) = app.stack.last_mut() {
                form.advanced = !form.advanced;
            }
            Command::Nothing
        }
        Command::EditGoal => push_field(app, "Goal", FieldKind::Goal),
        Command::EditThreadProfile => push_field(app, "Thread profile", FieldKind::ThreadProfile),
        Command::EditCoordinatorProfile => {
            push_field(app, "Coordinator profile", FieldKind::CoordinatorProfile)
        }
        Command::NewProfile => {
            app.stack.push(Layer::Field(FieldForm {
                title: "New profile name".into(),
                value: String::new(),
                kind: FieldKind::ProfileName,
            }));
            Command::Nothing
        }
        Command::RemoveProfile => {
            app.stack.push(Layer::Field(FieldForm {
                title: "Profile to delete".into(),
                value: String::new(),
                kind: FieldKind::ProfileRemove,
            }));
            Command::Nothing
        }
        Command::Archive => confirm(app, "Archive this project? Its folder stays. y/N", command),
        Command::Unarchive => confirm(app, "Unarchive this project? y/N", command),
        Command::RenameStart => push_field(app, "New slug", FieldKind::Rename),
        Command::SweepPreview | Command::SweepYes | Command::Rename | Command::CopyPath(_) => {
            command
        }
        Command::Delete => confirm(
            app,
            "Delete this project? Its folder moves to the trash. y/N",
            command,
        ),
        Command::YoloOn => confirm(
            app,
            "Turn yolo on? Threads start without asking, and agents skip permission prompts. y/N",
            command,
        ),
        Command::Resolve => confirm(
            app,
            "Resolve this thread and clean up its worktree, panes and merged branch? y/N",
            command,
        ),
        Command::ConfirmYes => confirm_yes(app),
        Command::ReadLibrary | Command::Notes => {
            app.stack.push(Layer::Detail);
            command
        }
        Command::InboxDone(_) => command,
        Command::OpenSafety => {
            app.safety_list = ListPos::default();
            app.stack.push(Layer::Safety);
            Command::Nothing
        }
        Command::SafetyKey(key) => open_safety_key(app, snap, &key),
        Command::SetSafety { key, value } => {
            if matches!(app.stack.last(), Some(Layer::SafetyPick(_))) {
                app.stack.pop();
            }
            if key == "yolo" && value == "on" {
                confirm(
                    app,
                    "Turn yolo on? Threads start without asking, and agents skip permission prompts. y/N",
                    Command::SetSafety { key, value },
                )
            } else {
                Command::SetSafety { key, value }
            }
        }
        other => other,
    }
}

fn open_safety_key(app: &mut App, snap: &Snapshot, key: &str) -> Command {
    let Some(value) = app.card(snap).and_then(|card| {
        card.safety
            .iter()
            .find(|row| row.key == key)
            .map(|row| row.value.clone())
    }) else {
        return Command::Nothing;
    };
    if key == "coordinator_agent_args" || key == "thread_agent_args" {
        let value = if value == "(none)" {
            String::new()
        } else {
            value
        };
        app.stack.push(Layer::Field(FieldForm {
            title: key.to_string(),
            value,
            kind: FieldKind::SafetyArgs,
        }));
        return Command::Nothing;
    }
    let Some(options) = safety_choices(key, &value) else {
        return Command::Nothing;
    };
    app.stack.push(Layer::SafetyPick(SafetyPick {
        key: key.to_string(),
        options,
        selected: 0,
    }));
    Command::Nothing
}

fn safety_choices(key: &str, value: &str) -> Option<Vec<String>> {
    let mut options = match key {
        "yolo" | "routine_commands" if value == "on" => vec!["off".into(), "on".into()],
        "yolo" | "routine_commands" => vec!["on".into(), "off".into()],
        "start_threads" if value == "auto" => vec!["propose".into(), "auto".into()],
        "start_threads" => vec!["auto".into(), "propose".into()],
        "trust_screens" if value == "coordinator" => {
            vec!["user".into(), "coordinator".into()]
        }
        "trust_screens" => vec!["coordinator".into(), "user".into()],
        _ => return None,
    };
    options.push("default".into());
    Some(options)
}

fn confirm(app: &mut App, text: &str, follow: Command) -> Command {
    app.stack.push(Layer::Confirm {
        text: text.into(),
        follow: Box::new(follow),
    });
    Command::Nothing
}

fn confirm_yes(app: &mut App) -> Command {
    let Some(Layer::Confirm { follow, .. }) = app.stack.pop() else {
        return Command::Nothing;
    };
    *follow
}

fn push_field(app: &mut App, title: &str, kind: FieldKind) -> Command {
    app.stack.push(Layer::Field(FieldForm {
        title: title.into(),
        value: String::new(),
        kind,
    }));
    Command::Nothing
}

fn submit_prompt(app: &mut App, snap: &Snapshot) -> Command {
    let Some(id) = app.project_id(snap) else {
        return Command::Nothing;
    };
    let prompt = app.prompt_mut(&id);
    match coordinator::begin_prompt_submit(prompt.field) {
        Some(field) => {
            if prompt.draft.trim().is_empty() {
                prompt.field = PromptField::Refused;
                app.notices.insert(id, "refused: the text is empty".into());
                return Command::Nothing;
            }
            prompt.field = field;
            Command::SubmitPrompt
        }
        None => {
            app.notices.insert(id, "not sent again".into());
            Command::Nothing
        }
    }
}

fn submit_field_step(app: &mut App) -> Command {
    let Some(Layer::Field(form)) = app.stack.last() else {
        return Command::Nothing;
    };
    if form.kind == FieldKind::Rename {
        let slug = form.value.trim().to_string();
        if slug.is_empty() {
            return Command::Nothing;
        }
        return confirm(
            app,
            &format!("Rename this project to {slug}? y/N"),
            Command::Rename,
        );
    }
    if form.kind == FieldKind::ProfileName {
        let name = form.value.trim().to_string();
        if name.is_empty() {
            return Command::Nothing;
        }
        app.profile_name = name;
        let agent = app.profile_agent.clone();
        app.stack.pop();
        app.stack.push(Layer::Field(FieldForm {
            title: "Profile agent".into(),
            value: agent,
            kind: FieldKind::ProfileAgent,
        }));
        return Command::Nothing;
    }
    Command::SubmitField
}

fn submit_new(app: &mut App) -> Command {
    let Some(Layer::New(form)) = app.stack.last() else {
        return Command::Nothing;
    };
    if form.name.trim().is_empty() {
        if let Some(Layer::New(form)) = app.stack.last_mut() {
            form.error = "the name is empty".into();
        }
        return Command::Nothing;
    }
    Command::SubmitNew
}

fn submit_thread(app: &mut App) -> Command {
    let Some(Layer::Thread(form)) = app.stack.last() else {
        return Command::Nothing;
    };
    if form.allocated.is_some() {
        return Command::Restart;
    }
    if form.title.trim().is_empty() {
        if let Some(Layer::Thread(form)) = app.stack.last_mut() {
            form.error = "the title is empty".into();
        }
        return Command::Nothing;
    }
    Command::SubmitThread
}

fn insert_char(app: &mut App, snap: &Snapshot, ch: char) {
    if let Some(layer) = app.stack.last_mut() {
        match layer {
            Layer::New(form) => form_insert(form_field(form), ch),
            Layer::Thread(form) => form_insert(thread_field(form), ch),
            Layer::Field(form) => form.value.push(ch),
            _ => {}
        }
        return;
    }
    if app.focus == Focus::Prompt
        && let Some(id) = app.project_id(snap)
    {
        let prompt = app.prompt_mut(&id);
        prompt.field = coordinator::note_prompt_edit(prompt.field);
        if prompt.field == PromptField::Draft {
            prompt.draft.push(ch);
        }
    }
}

fn backspace(app: &mut App, snap: &Snapshot) {
    if let Some(layer) = app.stack.last_mut() {
        match layer {
            Layer::New(form) => {
                form_field(form).pop();
            }
            Layer::Thread(form) => {
                thread_field(form).pop();
            }
            Layer::Field(form) => {
                form.value.pop();
            }
            _ => {}
        }
        return;
    }
    if app.focus == Focus::Prompt
        && let Some(id) = app.project_id(snap)
    {
        let prompt = app.prompt_mut(&id);
        prompt.field = coordinator::note_prompt_edit(prompt.field);
        if prompt.field == PromptField::Draft {
            prompt.draft.pop();
        }
    }
}

fn form_field(form: &mut NewForm) -> &mut String {
    match form.field {
        0 => &mut form.name,
        1 => &mut form.goal,
        2 => &mut form.repos,
        3 => &mut form.thread_profile,
        _ => &mut form.coordinator_profile,
    }
}

fn thread_field(form: &mut ThreadForm) -> &mut String {
    match form.field {
        0 => &mut form.title,
        1 => &mut form.task,
        2 => &mut form.repo,
        3 => &mut form.profile,
        4 => &mut form.machine,
        _ => &mut form.pane,
    }
}

fn form_insert(field: &mut String, ch: char) {
    field.push(ch);
}

fn next_focus(focus: Focus, _cols: Columns, forward: bool) -> Focus {
    let mut order = vec![
        Focus::Global,
        Focus::Projects,
        Focus::Work,
        Focus::Tabs,
        Focus::Overview,
    ];
    if focus == Focus::Prompt {
        order.push(Focus::Prompt);
    }
    let index = order.iter().position(|item| *item == focus).unwrap_or(0);
    let len = order.len() as i32;
    let next = if forward {
        (index as i32 + 1) % len
    } else {
        (index as i32 - 1 + len) % len
    };
    order[next as usize]
}

fn open_attention(app: &mut App, snap: &Snapshot, project: usize, thread_id: &str) {
    if project < snap.projects.len() {
        app.project_ix = project;
        app.side = Side::All;
    }
    let Some(card) = app.card(snap) else {
        return;
    };
    let id = card.id.clone();
    if let Some(index) = crate::screen::compose::display_index(card, thread_id) {
        let len = card.threads.len();
        let nav = app.nav_mut(&id);
        nav.overview = false;
        nav.tab = 0;
        nav.threads.set(index, len, 8);
        nav.detail = ListPos::default();
    }
    app.focus = Focus::Overview;
    app.one = Focus::Overview;
    app.stack.push(Layer::Detail);
}

fn move_list(app: &mut App, snap: &Snapshot, delta: i32) {
    let window = 8;
    let which = match app.stack.last() {
        Some(Layer::New(_)) => 1,
        Some(Layer::Thread(_)) => 2,
        Some(Layer::Menu) => 3,
        Some(Layer::Picker) => 4,
        Some(Layer::Detail) => 5,
        Some(Layer::Safety) => 8,
        Some(Layer::SafetyPick(_)) => 9,
        Some(_) => 6,
        None => 0,
    };
    if which != 0 {
        match which {
            1 => {
                if let Some(Layer::New(form)) = app.stack.last_mut() {
                    let fields = if form.advanced { 5 } else { 3 };
                    form.field = step(form.field, delta, fields);
                }
            }
            2 => {
                if let Some(Layer::Thread(form)) = app.stack.last_mut() {
                    form.field = step(form.field, delta, 6);
                }
            }
            3 => app.menu_list.move_by(delta, menu_len(), window),
            4 => app.picker_list.move_by(
                delta,
                side_len(snap, app.side, app.filter.as_deref()),
                window,
            ),
            5 => {
                if let Some(id) = current_id(app, snap) {
                    let len =
                        detail_count(app.card(snap), app.nav.get(&id).unwrap_or(&Nav::default()));
                    if let Some(nav) = app.nav.get_mut(&id) {
                        nav.detail.move_by(delta, len, window);
                    }
                }
            }
            8 => {
                let len = app.card(snap).map(|card| card.safety.len()).unwrap_or(0);
                app.safety_list.move_by(delta, len, window);
            }
            9 => {
                if let Some(Layer::SafetyPick(pick)) = app.stack.last_mut() {
                    pick.selected = step(pick.selected, delta, pick.options.len());
                }
            }
            _ => {}
        }
        return;
    }
    match app.focus {
        Focus::Global | Focus::Projects => {
            app.side_list.move_by(
                delta,
                side_len(snap, app.side, app.filter.as_deref()),
                window,
            );
        }
        Focus::Work => {
            if let Some(id) = current_id(app, snap) {
                let len = work_len(app.card(snap));
                app.nav_mut(&id).work.move_by(delta, len, window);
            }
        }
        Focus::Tabs => {
            if let Some(id) = current_id(app, snap) {
                let nav = app.nav_mut(&id);
                let index = if nav.overview { 0 } else { nav.tab + 1 };
                let next = step(index, delta, 7);
                if next == 0 {
                    nav.overview = true;
                } else {
                    nav.overview = false;
                    nav.tab = next - 1;
                }
            }
        }
        Focus::Overview => {
            if let Some(id) = current_id(app, snap) {
                let len = crate::screen::compose::feature_len(app, snap);
                if let Some(nav) = app.nav.get_mut(&id) {
                    list_for_tab(nav).move_by(delta, len, window);
                }
            }
        }
        Focus::Prompt => {}
    }
}

fn current_id(app: &App, snap: &Snapshot) -> Option<String> {
    app.project_id(snap)
}

fn step(index: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = index as i32 + delta;
    if next < 0 {
        len - 1
    } else {
        (next as usize) % len
    }
}

pub fn work_len(card: Option<&Card>) -> usize {
    coordinator_actions(card).len()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatorState {
    Missing,
    Stale,
    Ineligible,
    Recorded,
}

pub fn coordinator_state(line: &str) -> CoordinatorState {
    if line.is_empty() || line.contains("none recorded") {
        CoordinatorState::Missing
    } else if line.contains("stale") {
        CoordinatorState::Stale
    } else if line.contains("no socket") || line.contains("pane none") {
        CoordinatorState::Ineligible
    } else {
        CoordinatorState::Recorded
    }
}

pub fn coordinator_actions(card: Option<&Card>) -> Vec<(String, Command)> {
    let line = card.map(|card| card.coordinator.as_str()).unwrap_or("");
    let mut actions = Vec::new();
    match coordinator_state(line) {
        CoordinatorState::Missing => {
            actions.push(("Start coordinator".into(), Command::StartCoordinator));
        }
        CoordinatorState::Stale => {
            actions.push((
                "Inspect stale coordinator".into(),
                Command::InspectCoordinator,
            ));
        }
        CoordinatorState::Ineligible => {
            actions.push(("Inspect coordinator".into(), Command::InspectCoordinator));
        }
        CoordinatorState::Recorded => {
            actions.push((
                "Open coordinator in Herdr".into(),
                Command::OpenConversation,
            ));
        }
    }
    actions.push(("Send instruction".into(), Command::FocusPrompt));
    if card.is_some_and(|card| card.setup_incomplete) {
        actions.push(("Finish setup".into(), Command::FinishSetup));
    }
    actions
}

pub fn showing_overview(nav: Option<&Nav>) -> bool {
    nav.map(|nav| nav.overview).unwrap_or(true)
}

fn list_for_tab(nav: &mut Nav) -> &mut ListPos {
    if nav.overview {
        return &mut nav.overview_pos;
    }
    match nav.tab {
        0 => &mut nav.threads,
        1 => &mut nav.tasks,
        2 => &mut nav.library,
        3 => &mut nav.prs,
        4 => &mut nav.routines,
        _ => &mut nav.resources,
    }
}

pub fn tab_list(nav: &Nav) -> &ListPos {
    if nav.overview {
        return &nav.overview_pos;
    }
    match nav.tab {
        0 => &nav.threads,
        1 => &nav.tasks,
        2 => &nav.library,
        3 => &nav.prs,
        4 => &nav.routines,
        _ => &nav.resources,
    }
}

pub fn note_thread_allocated(app: &mut App, id: &str) {
    if let Some(Layer::Thread(form)) = app.stack.last_mut() {
        form.allocated = Some(id.to_string());
        form.error = format!("thread {id} was allocated; restart that thread");
    }
}

pub fn apply_prompt_end(app: &mut App, project_id: &str, text: &str, end: &str) {
    let prompt = app.prompt_mut(project_id);
    prompt.field = match end {
        "confirmed" => PromptField::Confirmed,
        "uncertain" => PromptField::Uncertain,
        "submitting" => PromptField::Submitting,
        _ => PromptField::Refused,
    };
    let label = match end {
        "confirmed" => "confirmed: herdr saw the agent working or blocked",
        "uncertain" => "uncertain: not sent again",
        "submitting" => "submitting",
        other => other,
    };
    prompt.outgoing.push((text.to_string(), label.to_string()));
    if end == "confirmed" {
        prompt.draft.clear();
    }
    app.notices
        .insert(project_id.to_string(), label.to_string());
}
