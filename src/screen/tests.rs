use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::project::{self, NewProject, Repo};
use crate::screen::compose::{self, Placed};
use crate::screen::draw;
use crate::screen::input;
use crate::screen::load::{self, Snapshot};
use crate::screen::state::{self, App, Columns, Command, Focus, Side};
use crate::thread::{self, Kind, Status};

struct Fixture {
    _root: tempfile::TempDir,
    _config: tempfile::TempDir,
    snap: Snapshot,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let repos = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("config.toml"),
        "\
[profiles.claude]
agent = \"claude\"

[profiles.codex]
agent = \"codex\"

[profiles.grok]
agent = \"grok\"

[profiles.pi]
agent = \"pi\"
",
    )
    .unwrap();
    let mut repo_paths = Vec::new();
    for (index, machine) in [
        ("box-a", true),
        ("box-b", true),
        ("box-c", true),
        ("local", false),
    ] {
        let path = repos.path().join(format!(
            "very-long-repository-path-number-{index}-that-stays-in-the-project"
        ));
        std::fs::create_dir_all(&path).unwrap();
        repo_paths.push(Repo {
            path: path.display().to_string(),
            machine: machine.then(|| index.to_string()),
        });
    }
    let horizon = project::create_with_setup(
        root.path(),
        &NewProject {
            name: "Horizon".into(),
            goal: "Ship the horizon goal".into(),
            repos: repo_paths,
            thread_profile: "claude".into(),
            coordinator_profile: "claude".into(),
            prefix: "hp --root /tmp".into(),
        },
    )
    .unwrap();
    project::create_with_setup(
        root.path(),
        &NewProject {
            name: "Other".into(),
            goal: "Second".into(),
            repos: Vec::new(),
            thread_profile: "claude".into(),
            coordinator_profile: "claude".into(),
            prefix: "hp --root /tmp".into(),
        },
    )
    .unwrap();
    horizon
        .update_coordinator(|record| {
            record.socket = "/no/such/herdr.sock".into();
            record.pane_id = "w1:p1".into();
            record.agent_name = "hpc-horizon".into();
            record.profile = "claude".into();
            record.cwd = horizon.dir().display().to_string();
        })
        .unwrap();
    let one = root.path().join("wt-one");
    let two = root.path().join("wt-two");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    for index in 0..24 {
        let title = if index == 0 {
            "LongTitleProbe the coordinator binding and the repository path that stays on screen"
                .to_string()
        } else if index == 1 {
            "Fix".into()
        } else {
            format!("Task {index}")
        };
        thread::allocate(&horizon, |thread| {
            thread.title = title;
            if index == 5 {
                thread.profile = "codex".into();
            }
            thread.kind = if index == 5 {
                Kind::Tab
            } else if index == 6 {
                Kind::Checkout
            } else {
                Kind::Worktree
            };
            if index == 0 {
                thread.worktree_path = one.display().to_string();
                thread.status = Status::Open;
            }
            if index == 1 {
                thread.pr = "https://github.com/acme/app/pull/4".into();
                thread.status = Status::Open;
            }
            if index == 2 {
                thread.pane_id = "w2:p2".into();
                thread.status = Status::Open;
                thread.worktree_path = two.display().to_string();
            }
            if index == 3 {
                thread.pane_id = "w2:p3".into();
                thread.status = Status::Failed;
            }
            if index == 7 {
                thread.machine = "box-a".into();
            }
        })
        .unwrap();
    }
    std::fs::write(
        thread::home_report_path(&horizon, "t-0001"),
        "shipped the note\n",
    )
    .unwrap();
    let (store, row) = horizon.open_row().unwrap();
    store
        .start_intent(
            &row.id,
            "thread_start",
            r#"{"target_id":"t-0002","intent":"thread_start"}"#,
        )
        .unwrap();
    let gamma = root.path().join("gamma");
    std::fs::create_dir_all(gamma.join(".state")).unwrap();
    std::fs::write(gamma.join("PROJECT.md"), "+++\nname = \"Gamma\"\n+++\n\n").unwrap();
    std::fs::write(gamma.join(".state").join("project.json"), "not json").unwrap();
    let snap = load::load(root.path(), config.path()).unwrap();
    Fixture {
        _root: root,
        _config: config,
        snap,
    }
}

fn horizon(snap: &Snapshot) -> &load::Card {
    snap.projects
        .iter()
        .find(|card| card.slug == "horizon")
        .unwrap()
}

fn app_on(snap: &Snapshot) -> App {
    let mut app = App::default();
    app.select_slug(snap, "horizon");
    app
}

fn frame(app: &App, snap: &Snapshot, width: u16, height: u16) -> String {
    let first = draw::frame_text(app, snap, width, height);
    assert_eq!(first, draw::frame_text(app, snap, width, height));
    first
}

fn save_frame(name: &str, text: &str) {
    let Ok(dir) = std::env::var("HERDR_FRAME_DIR") else {
        return;
    };
    let path = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join(name), text).unwrap();
}

fn line_matching(
    app: &App,
    snap: &Snapshot,
    width: u16,
    height: u16,
    pred: impl Fn(&Placed) -> bool,
) -> Placed {
    compose::plan(app, snap, width, height)
        .into_iter()
        .find(pred)
        .expect("placed line")
}

#[test]
fn contract_frames_at_the_layout_breakpoints() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let card = horizon(snap);
    assert_eq!(card.threads.len(), 24);
    assert_eq!(
        card.resources
            .iter()
            .filter(|row| row.kind == "repository")
            .count(),
        4
    );
    assert_eq!(
        card.resources
            .iter()
            .filter(|row| row.kind == "machine")
            .count(),
        3
    );
    assert_eq!(
        card.resources
            .iter()
            .filter(|row| row.label.starts_with("harness profile"))
            .count(),
        4
    );
    assert!(card.coordinator.contains("stale: socket missing"));
    assert!(card.prs.iter().any(|pr| pr.reference.contains("/pull/4")));
    assert!(
        card.operations
            .iter()
            .any(|line| line.contains("pending") && line.contains("thread_start"))
    );
    assert!(card.recovery.iter().any(|line| line.contains("unresolved")));
    assert!(snap.failed.iter().any(|failed| {
        failed.path.ends_with("gamma") && failed.message.contains("does not parse")
    }));
    assert!(snap.projects.iter().all(|card| card.id.starts_with("prj_")));
    let threads = thread::list(&project::Project::load(fixture._root.path(), "horizon").unwrap());
    assert!(
        threads
            .iter()
            .any(|thread| thread.worktree_path.ends_with("wt-one"))
    );
    assert!(
        threads
            .iter()
            .any(|thread| thread.worktree_path.ends_with("wt-two"))
    );
    assert!(threads.iter().any(|thread| thread.kind == Kind::Tab));
    assert!(threads.iter().any(|thread| thread.kind == Kind::Checkout));
    assert!(threads.iter().any(|thread| thread.status == Status::Failed));

    let mut app = app_on(snap);
    assert_eq!(state::columns(23), Columns::TooSmall);
    assert_eq!(state::columns(24), Columns::Stacked);
    assert_eq!(state::columns(80), Columns::Stacked);
    assert_eq!(state::columns(100), Columns::Three);
    assert_eq!(state::columns(160), Columns::Three);
    let sizes = [
        (50, 30),
        (60, 35),
        (70, 30),
        (80, 24),
        (100, 24),
        (120, 30),
        (160, 40),
    ];
    for (width, height) in sizes {
        let text = frame(&app, snap, width, height);
        assert!(text.contains("HERDR PROJECTS"), "{width}x{height}\n{text}");
        assert!(text.contains("horizon"), "{width}x{height}");
        assert!(text.contains("Other"), "{width}x{height}");
        assert!(text.contains("SELECTED"), "{width}x{height}");
        assert!(text.contains("New Project"), "{width}x{height}");
        assert!(
            text.contains("Inspect stale coordinator"),
            "{width}x{height}\n{text}"
        );
        assert!(
            text.contains("OVERVIEW") || text.contains("[OVERVIEW]"),
            "{width}x{height}"
        );
        assert!(!text.contains("Terminal is too small"), "{width}x{height}");
        assert!(text.contains("does not parse"), "{width}x{height}\n{text}");
        if matches!((width, height), (50, 30) | (70, 30) | (80, 24)) {
            assert!(
                text.contains("unresolved recovery"),
                "{width}x{height}\n{text}"
            );
            assert!(text.contains("thread_start"), "{width}x{height}\n{text}");
        }
        assert!(!text.contains("Draft:"), "{width}x{height}");
        assert!(!text.contains("ASK COORDINATOR"), "{width}x{height}");
        assert!(!text.contains("Project menu"), "{width}x{height}");
        assert!(!text.contains("repositories:"), "{width}x{height}");
        assert!(!text.contains("coordinator:"), "{width}x{height}");
        let tabs = compose::plan(&app, snap, width, height);
        let ys: Vec<u16> = tabs
            .iter()
            .filter(|line| matches!(line.command, Command::ShowOverview | Command::SelectTab(_)))
            .map(|line| line.y)
            .collect();
        assert!(!ys.is_empty(), "{width}x{height}");
        assert!(ys.iter().all(|y| *y == ys[0]), "{width}x{height} {ys:?}");
        for line in text.lines() {
            assert_eq!(line.chars().count(), width as usize, "{width}x{height}");
        }
        save_frame(&format!("{width}x{height}.txt"), &text);
    }
    let more = line_matching(&app, snap, 50, 30, |line| line.text == "[>]");
    let Command::SelectTab(index) = more.command else {
        panic!("{}", more.text);
    };
    assert_eq!(
        input::click_command(&app, snap, more.x, more.y, 50, 30),
        Command::SelectTab(index)
    );
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Char('g')), 50, 30),
        Command::Nothing
    );
    let digit = char::from(b'1' + index as u8);
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Char(digit)), 50, 30),
        Command::SelectTab(index)
    );
    let kept = app.project_id(snap).unwrap();
    state::apply(&mut app, Command::SelectTab(index), snap, 50);
    assert_eq!(app.project_id(snap).unwrap(), kept);
    assert_eq!(app.nav[&kept].tab, index);
    assert!(!app.nav[&kept].overview);
    app.nav.get_mut(&kept).unwrap().overview = true;
    app.focus = Focus::Work;
    let mut long = snap.clone();
    let horizon_card = long
        .projects
        .iter_mut()
        .find(|card| card.slug == "horizon")
        .unwrap();
    horizon_card.goal =
        "Finish the SQLAlchemy 2.0 adoption in partners. The full goal stays readable on a phone."
            .into();
    horizon_card.name = format!("LongName{}", "Z".repeat(80));
    let narrow_name = frame(&app, &long, 50, 30);
    assert!(narrow_name.contains("LongName"));
    assert!(narrow_name.contains("Finish the SQLAlchemy"));
    assert!(!narrow_name.contains("phone."));
    assert!(!narrow_name.contains(&"Z".repeat(40)));
    for line in narrow_name.lines() {
        assert_eq!(line.chars().count(), 50);
    }
    let mid = frame(&app, snap, 120, 30);
    assert!(mid.contains("New Project"));
    assert!(mid.contains("horizon"));
    let overview = frame(&app, snap, 160, 40);
    assert!(overview.contains("GOAL"));
    assert!(overview.contains("Stale coordinator"));
    assert!(overview.contains("Inspect stale coordinator"));
    assert!(overview.contains("Send instruction"));
    assert!(overview.contains("report t-0001:"));
    assert!(overview.contains("stale: socket missing"));
    assert!(overview.contains("unresolved recovery"));
    assert!(overview.contains("worktree identifiers were not stored"));
    assert!(overview.contains("nothing was attached"));
    assert!(overview.contains("pending"));
    assert!(overview.contains("thread_start"));
    assert!(overview.contains("[>]"));
    assert!(!overview.contains("coordinator:"));
    assert!(!overview.contains("Project menu"));
    assert!(!overview.contains("repositories:"));
    let placed = compose::plan(&app, snap, 160, 40);
    assert!(
        placed
            .iter()
            .any(|line| line.x < 31 && line.text.contains("Horizon"))
    );
    assert!(placed.iter().any(|line| {
        line.x < 31 && (line.text.contains("does not parse") || line.text.contains("project.json"))
    }));
    assert!(placed.iter().filter(|line| line.x < 31).all(|line| {
        !line.text.contains("GOAL")
            && !line.text.contains("COORDINATOR")
            && !line.text.contains("repositories:")
            && line.text != "Project menu"
    }));
    assert!(
        placed
            .iter()
            .filter(|line| (32..96).contains(&line.x))
            .all(|line| {
                !line.text.contains("unresolved")
                    && !line.text.contains("report t-")
                    && !line.text.starts_with("coordinator:")
                    && !line.text.contains("repositories:")
            })
    );
    assert!(
        placed
            .iter()
            .any(|line| line.x >= 96 && line.text.contains("unresolved recovery"))
    );
    state::apply(&mut app, Command::SelectTab(0), snap, 160);
    let threads = frame(&app, snap, 160, 40);
    assert!(threads.contains("LongTitleProbe"), "{threads}");
    state::apply(&mut app, Command::SelectTab(3), snap, 160);
    let prs = frame(&app, snap, 160, 40);
    assert!(prs.contains("recorded reference"), "{prs}");
    assert!(prs.contains("not a live check"), "{prs}");
    assert!(prs.contains("SELECTED / Horizon"), "{prs}");
    state::apply(&mut app, Command::SelectTab(5), snap, 160);
    let resources = frame(&app, snap, 160, 40);
    assert!(resources.contains("repository "), "{resources}");
    assert!(resources.contains("harness profile"), "{resources}");
    assert!(!resources.contains("repositories:"), "{resources}");
    let id = app.project_id(snap).unwrap();
    app.prompt_mut(&id).outgoing.push((
        "look here".into(),
        "confirmed: herdr saw the agent working or blocked".into(),
    ));
    let wide = frame(&app, snap, 160, 40);
    assert!(wide.contains("you sent: look here"));
    assert!(wide.contains("Inspect stale coordinator"));
    assert!(wide.contains("Send instruction"));
    assert!(!wide.contains("replied"));
    assert!(!wide.contains("Accepted"));
    assert!(!wide.contains("w9:p9"));
}

#[test]
fn contract_cockpit_selects_a_project_without_leaving_the_portfolio() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut preset = App::default();
    preset.select_slug(snap, "other");
    let preset_text = frame(&preset, snap, 80, 24);
    assert!(preset_text.contains("SELECTED / Other"), "{preset_text}");
    assert!(preset_text.contains("Horizon"), "{preset_text}");
    assert!(preset_text.contains("Start coordinator"), "{preset_text}");
    assert!(preset.pending_focus.is_none());

    let mut app = app_on(snap);
    assert!(app.pending_focus.is_none());
    let before = frame(&app, snap, 120, 30);
    assert!(before.contains("SELECTED / Horizon"), "{before}");
    assert!(app.pending_focus.is_none());
    assert!(app.stack.is_empty());

    let other = snap
        .projects
        .iter()
        .position(|card| card.slug == "other")
        .unwrap();
    state::apply(&mut app, Command::SelectProject(other), snap, 120);
    let selected = frame(&app, snap, 60, 35);
    assert!(selected.contains("SELECTED / Other"), "{selected}");
    assert!(selected.contains("Start coordinator"), "{selected}");
    assert!(selected.contains("No coordinator recorded"), "{selected}");
    assert!(selected.contains("Start first Thread"), "{selected}");
    assert!(selected.contains("Horizon"), "{selected}");
    assert!(!selected.contains("no report"), "{selected}");
    assert!(!selected.contains("no recovery"), "{selected}");
    assert!(!selected.contains("NEEDS ATTENTION"), "{selected}");
    assert_eq!(app.project_id(snap).unwrap(), snap.projects[other].id);
    state::apply(&mut app, Command::SelectTab(1), snap, 60);
    assert_eq!(app.project_id(snap).unwrap(), snap.projects[other].id);
    assert_eq!(app.nav[&snap.projects[other].id].tab, 1);
    assert!(!app.nav[&snap.projects[other].id].overview);

    let mut recorded = snap.clone();
    recorded.projects[0].coordinator =
        "coordinator: socket present pane w1:p1 hpc-horizon profile claude".into();
    state::apply(&mut app, Command::SelectProject(0), &recorded, 120);
    let open = line_matching(&app, &recorded, 120, 30, |line| {
        line.text.starts_with("Open coordinator in Herdr")
    });
    assert_eq!(open.command, Command::OpenConversation);
    let mut blocked = snap.clone();
    blocked.projects[0].coordinator =
        "coordinator: no socket pane none unnamed profile claude".into();
    let inspect = line_matching(&app, &blocked, 120, 30, |line| {
        line.text.starts_with("Inspect coordinator") && !line.text.contains("stale")
    });
    assert_eq!(inspect.command, Command::InspectCoordinator);

    assert!(
        !snap.needs.is_empty(),
        "the fixture records thread attention"
    );
    let row = &snap.needs[0];
    let project = snap
        .projects
        .iter()
        .position(|card| card.id == row.project_id)
        .unwrap();
    state::apply(
        &mut app,
        Command::OpenAttention {
            project,
            thread_id: row.thread_id.clone(),
        },
        snap,
        120,
    );
    assert_eq!(app.project_ix, project);
    assert_eq!(app.side, Side::All);
    let card = &snap.projects[project];
    let nav = &app.nav[&card.id];
    assert!(!nav.overview);
    assert_eq!(nav.tab, 0);
    assert_eq!(
        nav.threads.selected,
        compose::display_index(card, &row.thread_id).unwrap()
    );
    assert!(matches!(app.stack.last(), Some(state::Layer::Detail)));

    app.stack.clear();
    app.focus = Focus::Projects;
    app.side = Side::All;
    let len = state::side_len(snap, app.side, None);
    app.side_list.selected = len - 1;
    let scrolled = frame(&app, snap, 50, 30);
    assert!(scrolled.contains("does not parse"), "{scrolled}");
    assert!(scrolled.contains("SELECTED"), "{scrolled}");
    assert!(scrolled.contains("New Project"), "{scrolled}");
    assert!(!scrolled.contains("Project menu"), "{scrolled}");
    let short = frame(&app, snap, 50, 16);
    assert!(short.contains("does not parse"), "{short}");
    assert!(short.contains("SELECTED"), "{short}");
    assert!(!short.contains("New Project"), "{short}");
    assert!(!short.contains("Project menu"), "{short}");

    let mut phone_snap = snap.clone();
    phone_snap.failed.clear();
    phone_snap.needs.clear();
    phone_snap.inbox.clear();
    phone_snap.projects.retain(|card| card.slug == "other");
    let mut phone_app = App::default();
    phone_app.select_slug(&phone_snap, "other");
    let phone = compose::plan(&phone_app, &phone_snap, 72, 40);
    let selected_y = phone
        .iter()
        .find(|line| line.text.starts_with("SELECTED"))
        .unwrap()
        .y;
    let last_above = phone
        .iter()
        .filter(|line| line.y < selected_y)
        .map(|line| line.y)
        .max()
        .unwrap();
    assert_eq!(last_above + 1, selected_y);
    assert!(phone.iter().any(|line| line.text.starts_with("> Other ")));
    assert!(phone.iter().any(|line| line.text == "[Resources]"));
    assert!(!phone.iter().any(|line| line.text == "[>]"));
    let phone_text = frame(&phone_app, &phone_snap, 72, 40);
    save_frame("72x40.txt", &phone_text);
    assert!(phone_text.contains("Start first Thread"), "{phone_text}");
    assert!(phone_text.contains("New Project"), "{phone_text}");
    assert!(!phone_text.contains("no report"), "{phone_text}");
    assert!(!phone_text.contains("no recovery"), "{phone_text}");
    assert!(!phone_text.contains("NEEDS ATTENTION"), "{phone_text}");
    let work_y = phone.iter().find(|line| line.text == "WORK").unwrap().y;
    let start_y = phone
        .iter()
        .find(|line| line.text == "Start first Thread")
        .unwrap()
        .y;
    assert!(start_y < work_y + 8, "{phone_text}");
    assert!(
        phone
            .iter()
            .any(|line| line.text.starts_with("> Other missing"))
    );
    assert!(
        !phone
            .iter()
            .any(|line| line.text.starts_with("> Other ") && line.text.contains("0 open"))
    );
    phone_snap.projects[0].goal =
        "Finish the SQLAlchemy 2.0 adoption in partners. The full goal stays readable on a phone."
            .into();
    let wrapped = frame(&phone_app, &phone_snap, 72, 40);
    assert!(wrapped.contains("Finish the SQLAlchemy"), "{wrapped}");
    assert!(wrapped.contains("phone."), "{wrapped}");
    assert!(wrapped.contains("Start first Thread"), "{wrapped}");

    let mut counted = snap.clone();
    counted.inbox.push(load::InboxRow {
        project_id: card.id.clone(),
        slug: card.slug.clone(),
        id: "in-cockpit".into(),
        summary: "cockpit inbox".into(),
        body: "body".into(),
    });
    let counts = frame(&app, &counted, 160, 40);
    assert!(counts.contains(&format!("{} Need you", counted.needs.len())));
    assert!(counts.contains(&format!("{} Inbox", counted.inbox.len())));
    assert_ne!(counted.needs.len(), counted.inbox.len());

    app.stack.clear();
    app.focus = Focus::Overview;
    for digit in 1..=6 {
        assert_eq!(
            input::key_command(&mut app, snap, input::key(KeyCode::Char('g')), 120, 30),
            Command::Nothing
        );
        let ch = char::from(b'0' + digit);
        assert_eq!(
            input::key_command(&mut app, snap, input::key(KeyCode::Char(ch)), 120, 30),
            Command::SelectTab((digit - 1) as usize)
        );
    }
    app.focus = Focus::Prompt;
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Char('o')), 120, 30),
        Command::Insert('o')
    );
    let too_narrow = frame(&app, snap, 23, 30);
    let too_short = frame(&app, snap, 50, 9);
    assert!(too_narrow.contains("Terminal is too small"));
    assert!(too_short.contains("Terminal is too small"));
}

#[test]
fn contract_work_actions_stay_reachable_on_a_short_pane() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    app.focus = Focus::Work;
    for (width, height) in [(40, 16), (50, 30), (80, 24), (100, 24)] {
        let text = frame(&app, snap, width, height);
        assert!(
            text.contains("Inspect stale coordinator"),
            "{width}x{height}\n{text}"
        );
        assert!(!text.contains("Draft:"), "{width}");
        assert!(!text.contains("Open coordinator in Herdr"), "{width}");
        let stale = line_matching(&app, snap, width, height, |line| {
            line.text.starts_with("Inspect stale coordinator")
        });
        assert_eq!(stale.command, Command::InspectCoordinator);
    }
    state::apply(&mut app, Command::Move(1), snap, 80);
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Enter), 80, 24),
        Command::FocusPrompt
    );
    let mut incomplete = snap.clone();
    incomplete
        .projects
        .iter_mut()
        .find(|card| card.slug == "horizon")
        .unwrap()
        .setup_incomplete = true;
    let text = frame(&app, &incomplete, 80, 24);
    assert!(text.contains("Finish setup"), "{text}");
    let finish = line_matching(&app, &incomplete, 80, 24, |line| {
        line.text.starts_with("Finish setup")
    });
    assert_eq!(finish.command, Command::FinishSetup);
}

#[test]
fn ctrl_c_quits_while_the_prompt_is_open() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    app.focus = Focus::Prompt;
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(
        input::key_command(&mut app, snap, key, 63, 41),
        Command::Quit
    );
}

#[test]
fn contract_an_inbox_click_marks_the_clicked_item() {
    let fixture = fixture();
    let mut snap = fixture.snap.clone();
    let project_id = horizon(&snap).id.clone();
    snap.inbox = vec![
        load::InboxRow {
            project_id: project_id.clone(),
            slug: "horizon".into(),
            id: "in-first".into(),
            summary: "first item".into(),
            body: "one".into(),
        },
        load::InboxRow {
            project_id,
            slug: "horizon".into(),
            id: "in-second".into(),
            summary: "second item".into(),
            body: "two".into(),
        },
    ];
    let mut app = app_on(&snap);
    app.side = Side::Inbox;
    app.focus = Focus::Projects;
    assert_eq!(app.side_list.selected, 0);
    let second = line_matching(&app, &snap, 160, 40, |line| {
        line.text.starts_with("second item")
    });
    let clicked = input::click_command(&app, &snap, second.x, second.y, 160, 40);
    assert_eq!(clicked, Command::InboxDone("in-second".into()));
    let built = crate::screen::jobs::build(&app, &snap, &clicked, Some("hp")).unwrap();
    assert_eq!(built.inbox_id, "in-second");
    let index = compose::side_rows(&app, &snap)
        .iter()
        .position(|(text, _)| text.starts_with("second item"))
        .unwrap();
    app.side_list.selected = index;
    let keyed = input::key_command(&mut app, &snap, input::key(KeyCode::Enter), 160, 40);
    assert_eq!(keyed, clicked);
    let built = crate::screen::jobs::build(&app, &snap, &keyed, Some("hp")).unwrap();
    assert_eq!(built.inbox_id, "in-second");
}

#[test]
fn contract_esc_in_the_prompt_returns_to_work() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    app.focus = Focus::Prompt;
    let id = app.project_id(snap).unwrap();
    app.prompt_mut(&id).draft = "keep me".into();
    let command = on_key(&mut app, snap, KeyCode::Esc);
    assert_ne!(command, Command::Quit);
    assert_eq!(app.focus, Focus::Work);
    assert_eq!(app.prompts[&id].draft, "keep me");
    assert!(app.stack.is_empty());
    assert_eq!(on_key(&mut app, snap, KeyCode::Esc), Command::Quit);
}

#[test]
fn contract_keys_and_clicks_use_the_same_command() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    app.focus = Focus::Projects;
    let new_project = line_matching(&app, snap, 160, 40, |line| line.text == "New Project");
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Enter), 160, 40),
        new_project.command
    );
    assert_eq!(
        input::click_command(&app, snap, new_project.x, new_project.y, 160, 40),
        Command::OpenNew
    );

    app.focus = Focus::Work;
    let open = line_matching(&app, snap, 160, 40, |line| {
        line.text.starts_with("Inspect stale coordinator")
    });
    assert_eq!(open.command, Command::InspectCoordinator);
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Enter), 160, 40),
        open.command
    );
    assert_eq!(
        input::click_command(&app, snap, open.x, open.y, 160, 40),
        Command::InspectCoordinator
    );
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Char('o')), 160, 40),
        Command::InspectCoordinator
    );

    state::apply(&mut app, Command::SelectTab(0), snap, 160);
    let thread = line_matching(&app, snap, 160, 40, |line| {
        line.command == Command::Inspect && line.text.starts_with("t-0001")
    });
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Enter), 160, 40),
        thread.command
    );
    assert_eq!(
        input::click_command(&app, snap, thread.x, thread.y, 160, 40),
        Command::Inspect
    );

    state::apply(&mut app, Command::OpenNew, snap, 160);
    let cancel = line_matching(&app, snap, 160, 40, |line| {
        line.modal && line.text == "Cancel"
    });
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Esc), 160, 40),
        Command::Cancel
    );
    assert_eq!(
        input::click_command(&app, snap, cancel.x, cancel.y, 160, 40),
        Command::Cancel
    );
    assert_ne!(cancel.command, Command::SubmitNew);

    app.stack.clear();
    app.focus = Focus::Prompt;
    app.g = false;
    for ch in ['g', 'o', 'P', '1', '?'] {
        assert_eq!(
            input::key_command(&mut app, snap, input::key(KeyCode::Char(ch)), 160, 40),
            Command::Insert(ch)
        );
        assert!(!app.g);
    }

    let id = app.project_id(snap).unwrap();
    app.prompt_mut(&id).draft = "keep me".into();
    app.prompt_mut(&id).field = crate::coordinator::PromptField::Uncertain;
    assert_eq!(
        state::apply(&mut app, Command::SubmitPrompt, snap, 160),
        Command::Nothing
    );
    assert_eq!(app.prompts[&id].draft, "keep me");
    assert_eq!(
        app.prompts[&id].field,
        crate::coordinator::PromptField::Uncertain
    );
    assert_eq!(app.notices[&id], "not sent again");
}

#[test]
fn contract_selection_survives_project_switch_and_width() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    let command = input::key_command(&mut app, snap, input::key(KeyCode::Char('g')), 160, 40);
    assert_eq!(command, Command::Nothing);
    assert!(app.g);
    let command = input::key_command(&mut app, snap, input::key(KeyCode::Char('4')), 160, 40);
    assert_eq!(command, Command::SelectTab(3));
    state::apply(&mut app, command, snap, 160);
    app.focus = Focus::Overview;
    state::apply(&mut app, Command::SelectTab(0), snap, 160);
    for _ in 0..10 {
        state::apply(&mut app, Command::Move(1), snap, 160);
    }
    let id = app.project_id(snap).unwrap();
    let selected = app.nav[&id].threads.selected;
    let offset = app.nav[&id].threads.offset;
    assert_eq!(selected, 10);
    state::apply(&mut app, Command::SelectProject(1), snap, 160);
    state::apply(&mut app, Command::SelectTab(1), snap, 120);
    state::apply(&mut app, Command::SelectProject(0), snap, 100);
    assert_eq!(app.nav[&id].tab, 0);
    assert_eq!(app.nav[&id].threads.selected, selected);
    assert_eq!(app.nav[&id].threads.offset, offset);
    let other = snap
        .projects
        .iter()
        .find(|card| card.slug == "other")
        .unwrap();
    assert_eq!(app.nav[&other.id].tab, 1);
    let _ = frame(&app, snap, 100, 24);
    let _ = frame(&app, snap, 80, 24);
    assert_eq!(app.nav[&id].threads.selected, selected);
    assert_eq!(app.nav[&id].threads.offset, offset);
    assert_eq!(app.nav[&id].tab, 0);
}

fn press(app: &mut App, snap: &Snapshot, code: KeyCode) -> Command {
    input::key_command(app, snap, input::key(code), 160, 40)
}

#[test]
fn contract_overview_keys_menu_and_separate_counts() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    state::apply(&mut app, Command::SelectTab(0), snap, 160);
    assert_eq!(app.focus, Focus::Overview);
    assert_eq!(press(&mut app, snap, KeyCode::Char('a')), Command::Ack);
    assert_eq!(press(&mut app, snap, KeyCode::Char('s')), Command::Stop);
    assert_eq!(press(&mut app, snap, KeyCode::Char('r')), Command::Restart);
    assert_eq!(press(&mut app, snap, KeyCode::Char('x')), Command::Resolve);
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('t')),
        Command::StartThreadForm
    );
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('1')),
        Command::NextLine(0)
    );
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('o')),
        Command::OpenWorker
    );

    for digit in 1..=6 {
        assert_eq!(press(&mut app, snap, KeyCode::Char('g')), Command::Nothing);
        assert!(app.g);
        let ch = char::from(b'0' + digit);
        assert_eq!(
            press(&mut app, snap, KeyCode::Char(ch)),
            Command::SelectTab((digit - 1) as usize)
        );
        state::apply(
            &mut app,
            Command::SelectTab((digit - 1) as usize),
            snap,
            160,
        );
        let rows = compose::detail_rows(&app, snap);
        assert_eq!(rows.len(), 4);
    }
    let id = app.project_id(snap).unwrap();
    assert_eq!(app.nav[&id].tab, 5);
    state::apply(&mut app, Command::SelectTab(0), snap, 160);
    state::apply(&mut app, Command::Inspect, snap, 160);
    let rows = compose::detail_rows(&app, snap);
    assert_eq!(rows[1].1, Command::StartPr);
    assert_eq!(rows[2].1, Command::OpenPr);
    assert!(rows[0].0.contains("not a live check") || rows[0].0.contains("t-"));
    assert_eq!(press(&mut app, snap, KeyCode::Enter), rows[0].1);
    let restart = line_matching(&app, snap, 160, 40, |line| line.text == "Restart");
    assert_eq!(
        input::click_command(&app, snap, restart.x, restart.y, 160, 40),
        Command::Restart
    );
    assert_eq!(press(&mut app, snap, KeyCode::Esc), Command::Cancel);
    state::apply(&mut app, Command::Cancel, snap, 160);

    state::apply(&mut app, Command::SelectTab(1), snap, 160);
    assert_eq!(press(&mut app, snap, KeyCode::Char('d')), Command::Delegate);
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('c')),
        Command::CompleteTask
    );
    assert_eq!(press(&mut app, snap, KeyCode::Char('x')), Command::DropTask);
    let tasks = compose::detail_rows(&app, snap);
    assert_eq!(tasks[1].1, Command::Delegate);
    assert_eq!(tasks[2].1, Command::CompleteTask);
    assert_eq!(tasks[3].1, Command::DropTask);

    state::apply(&mut app, Command::SelectTab(2), snap, 160);
    assert_eq!(compose::detail_rows(&app, snap)[1].1, Command::ReadLibrary);
    state::apply(&mut app, Command::SelectTab(3), snap, 160);
    let prs = compose::detail_rows(&app, snap);
    assert!(prs[0].0.contains("not a live check") || prs[0].0.contains("no recorded"));
    assert_eq!(prs[1].1, Command::OpenPr);
    state::apply(&mut app, Command::SelectTab(4), snap, 160);
    assert_eq!(
        compose::detail_rows(&app, snap)[1].1,
        Command::ToggleRoutine
    );
    state::apply(&mut app, Command::SelectTab(5), snap, 160);
    assert_eq!(compose::detail_rows(&app, snap)[1].1, Command::OpenResource);

    app.stack.clear();
    app.focus = Focus::Projects;
    app.side_list
        .set(2, state::side_len(snap, app.side, None), 8);
    let needs = line_matching(&app, snap, 160, 40, |line| {
        line.text.starts_with("Needs you")
    });
    assert_eq!(press(&mut app, snap, KeyCode::Enter), needs.command);
    assert_eq!(
        input::click_command(&app, snap, needs.x, needs.y, 160, 40),
        Command::SelectSide(Side::Needs)
    );
    app.side_list
        .set(3, state::side_len(snap, app.side, None), 8);
    let inbox = line_matching(&app, snap, 160, 40, |line| line.text.starts_with("Inbox"));
    assert_eq!(press(&mut app, snap, KeyCode::Enter), inbox.command);
    assert_eq!(
        input::click_command(&app, snap, inbox.x, inbox.y, 160, 40),
        Command::SelectSide(Side::Inbox)
    );
    assert_ne!(needs.text, inbox.text);

    app.stack.clear();
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('P')),
        Command::OpenProjectSelection
    );
    state::apply(&mut app, Command::OpenProjectSelection, snap, 120);
    assert!(app.stack.is_empty());
    assert_eq!(app.focus, Focus::Projects);
    let picker = frame(&app, snap, 120, 30);
    assert!(picker.contains("New Project"));
    assert!(picker.contains("horizon"));

    state::apply(&mut app, Command::OpenProjectSelection, snap, 160);
    assert!(app.stack.is_empty());
    assert_eq!(app.focus, Focus::Projects);

    assert_eq!(press(&mut app, snap, KeyCode::Char('m')), Command::OpenMenu);
    state::apply(&mut app, Command::OpenMenu, snap, 160);
    for _ in 0..9 {
        state::apply(&mut app, Command::Move(1), snap, 160);
    }
    assert_eq!(press(&mut app, snap, KeyCode::Enter), Command::Delete);
    assert_eq!(
        state::apply(&mut app, Command::Delete, snap, 160),
        Command::Nothing
    );
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('y')),
        Command::ConfirmYes
    );
    assert_eq!(
        state::apply(&mut app, Command::ConfirmYes, snap, 160),
        Command::Delete
    );
    app.stack.clear();
    assert_eq!(
        state::apply(&mut app, Command::YoloOff, snap, 160),
        Command::YoloOff
    );
    assert!(app.stack.is_empty());
    assert_eq!(
        state::apply(&mut app, Command::YoloOn, snap, 160),
        Command::Nothing
    );
    assert_eq!(
        state::apply(&mut app, Command::ConfirmYes, snap, 160),
        Command::YoloOn
    );
    app.stack.clear();
    assert_eq!(
        state::apply(&mut app, Command::Archive, snap, 160),
        Command::Nothing
    );
    assert_eq!(press(&mut app, snap, KeyCode::Char('n')), Command::Cancel);
}

fn on_key(app: &mut App, snap: &Snapshot, code: KeyCode) -> Command {
    input::on_key(app, snap, input::key(code), 160, 40)
}

fn run_follow(
    ctx: &crate::paths::Ctx,
    app: &mut App,
    snap: &Snapshot,
    follow: Command,
) -> crate::screen::jobs::Outcome {
    let request = crate::screen::jobs::build(app, snap, &follow, Some("hp --root /tmp"))
        .expect("the command runs");
    let outcome = crate::screen::jobs::perform(ctx, &request);
    crate::screen::jobs::apply_outcome(app, &outcome);
    outcome
}

#[test]
fn contract_safety_menu_and_routine_prompt() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    let env = crate::paths::Env::for_test(fixture._root.path(), &[]);
    let runner = crate::runner::fake::FakeRunner::new();
    let ctx = crate::paths::Ctx {
        env: &env,
        root: fixture._root.path().to_path_buf(),
        config_dir: fixture._config.path().to_path_buf(),
        runner: &runner,
        detached_ticker: false,
    };

    assert_eq!(on_key(&mut app, snap, KeyCode::Char('m')), Command::Nothing);
    assert!(matches!(app.stack.last(), Some(state::Layer::Menu)));
    for _ in 0..10 {
        on_key(&mut app, snap, KeyCode::Down);
    }
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Enter), 160, 40),
        Command::OpenSafety
    );
    assert_eq!(on_key(&mut app, snap, KeyCode::Enter), Command::Nothing);
    assert!(matches!(app.stack.last(), Some(state::Layer::Safety)));
    let shown = compose::plan(&app, snap, 160, 40)
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(shown.contains("start_threads"), "{shown}");
    assert!(shown.contains("trust_screens"), "{shown}");
    assert!(shown.contains("routine_commands"), "{shown}");
    assert!(shown.contains("coordinator_agent_args"), "{shown}");
    assert!(shown.contains("thread_agent_args"), "{shown}");

    on_key(&mut app, snap, KeyCode::Down);
    let start = line_matching(&app, snap, 160, 40, |line| {
        line.text.contains("start_threads")
    });
    let key = input::key_command(&mut app, snap, input::key(KeyCode::Enter), 160, 40);
    assert_eq!(key, Command::SafetyKey("start_threads".into()));
    assert_eq!(
        input::click_command(&app, snap, start.x, start.y, 160, 40),
        key
    );
    assert_eq!(on_key(&mut app, snap, KeyCode::Enter), Command::Nothing);
    assert!(matches!(
        app.stack.last(),
        Some(state::Layer::SafetyPick(_))
    ));
    let pick = line_matching(&app, snap, 160, 40, |line| line.selected && line.modal);
    assert_eq!(
        pick.command,
        Command::SetSafety {
            key: "start_threads".into(),
            value: "auto".into(),
        }
    );
    assert!(
        compose::plan(&app, snap, 160, 40)
            .iter()
            .any(|line| line.text == "default")
    );
    let follow = on_key(&mut app, snap, KeyCode::Enter);
    let outcome = run_follow(&ctx, &mut app, snap, follow);
    assert_eq!(outcome.status, "done");
    assert!(matches!(app.stack.last(), Some(state::Layer::Safety)));

    on_key(&mut app, snap, KeyCode::Down);
    on_key(&mut app, snap, KeyCode::Down);
    let args = line_matching(&app, snap, 160, 40, |line| {
        line.text.contains("coordinator_agent_args")
    });
    assert_eq!(
        input::click_command(&app, snap, args.x, args.y, 160, 40),
        Command::SafetyKey("coordinator_agent_args".into())
    );
    assert_eq!(on_key(&mut app, snap, KeyCode::Enter), Command::Nothing);
    assert!(matches!(
        app.stack.last(),
        Some(state::Layer::Field(form)) if form.kind == state::FieldKind::SafetyArgs
    ));
    for ch in ['-', '-', 'f', 'l', 'a', 'g'] {
        on_key(&mut app, snap, KeyCode::Char(ch));
    }
    let follow = on_key(&mut app, snap, KeyCode::Enter);
    assert_eq!(follow, Command::SubmitField);
    let outcome = run_follow(&ctx, &mut app, snap, follow);
    assert_eq!(outcome.status, "done");
    assert!(matches!(app.stack.last(), Some(state::Layer::Safety)));

    for _ in 0..3 {
        on_key(&mut app, snap, KeyCode::Up);
    }
    assert_eq!(on_key(&mut app, snap, KeyCode::Enter), Command::Nothing);
    let follow = on_key(&mut app, snap, KeyCode::Enter);
    assert_eq!(follow, Command::Nothing);
    assert!(matches!(
        app.stack.last(),
        Some(state::Layer::Confirm { .. })
    ));
    let text = std::fs::read_to_string(fixture._config.path().join("config.toml")).unwrap();
    assert!(!text.contains("yolo"), "{text}");
    assert_eq!(on_key(&mut app, snap, KeyCode::Char('n')), Command::Nothing);
    assert!(matches!(app.stack.last(), Some(state::Layer::Safety)));
    assert_eq!(on_key(&mut app, snap, KeyCode::Enter), Command::Nothing);
    assert_eq!(on_key(&mut app, snap, KeyCode::Enter), Command::Nothing);
    let follow = on_key(&mut app, snap, KeyCode::Char('y'));
    assert!(matches!(
        follow,
        Command::SetSafety { ref key, ref value } if key == "yolo" && value == "on"
    ));
    let outcome = run_follow(&ctx, &mut app, snap, follow);
    assert_eq!(outcome.status, "done");

    let again = load::load(fixture._root.path(), fixture._config.path()).unwrap();
    let card = horizon(&again);
    let row = |key: &str| {
        card.safety
            .iter()
            .find(|item| item.key == key)
            .unwrap_or_else(|| panic!("missing {key}"))
    };
    assert_eq!(row("start_threads").value, "auto");
    assert_eq!(row("start_threads").source, "project");
    assert_eq!(row("coordinator_agent_args").value, "--flag");
    assert_eq!(row("coordinator_agent_args").source, "project");
    assert_eq!(row("yolo").value, "on");
    assert_eq!(row("yolo").source, "project");
    assert_eq!(row("routine_commands").value, "off");

    app.stack.clear();
    app.focus = Focus::Overview;
    assert_eq!(on_key(&mut app, snap, KeyCode::Char('g')), Command::Nothing);
    assert_eq!(on_key(&mut app, snap, KeyCode::Char('5')), Command::Nothing);
    assert_eq!(app.nav[&app.project_id(snap).unwrap()].tab, 4);
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Char('i')), 160, 40),
        Command::Inspect
    );
    assert_eq!(on_key(&mut app, snap, KeyCode::Char('i')), Command::Nothing);
    let rows = compose::detail_rows(&app, snap);
    assert_eq!(rows.len(), 4);
    assert!(
        rows[0].0.contains("Fix the failing checks"),
        "{}",
        rows[0].0
    );
    assert_eq!(rows[1].1, Command::ToggleRoutine);
    let prompt = line_matching(&app, snap, 160, 40, |line| {
        line.text.contains("Fix the failing")
    });
    assert_eq!(
        input::click_command(&app, snap, prompt.x, prompt.y, 160, 40),
        Command::Nothing
    );
    on_key(&mut app, snap, KeyCode::Down);
    let toggle = line_matching(&app, snap, 160, 40, |line| line.text == "Toggle routine");
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Enter), 160, 40),
        Command::ToggleRoutine
    );
    assert_eq!(
        input::click_command(&app, snap, toggle.x, toggle.y, 160, 40),
        Command::ToggleRoutine
    );
}

fn on_width(app: &mut App, snap: &Snapshot, code: KeyCode, width: u16) -> Command {
    input::on_key(app, snap, input::key(code), width, 30)
}

#[test]
fn contract_filter_sweep_rename_unarchive_and_copy() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    let env = crate::paths::Env::for_test(fixture._root.path(), &[]);
    let runner = crate::runner::fake::FakeRunner::new();
    let ctx = crate::paths::Ctx {
        env: &env,
        root: fixture._root.path().to_path_buf(),
        config_dir: fixture._config.path().to_path_buf(),
        runner: &runner,
        detached_ticker: false,
    };

    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Char('/')), 120, 30),
        Command::StartFilter
    );
    assert_eq!(
        on_width(&mut app, snap, KeyCode::Char('/'), 120),
        Command::Nothing
    );
    assert_eq!(app.filter.as_deref(), Some(""));
    assert!(app.stack.is_empty());
    assert_eq!(app.focus, Focus::Projects);
    for ch in ['o', 't', 'h'] {
        on_width(&mut app, snap, KeyCode::Char(ch), 120);
    }
    let rows = compose::side_rows(&app, snap);
    assert!(rows.iter().any(|(text, _)| text.starts_with("Other ")));
    assert!(!rows.iter().any(|(text, _)| text.starts_with("Horizon ")));
    for _ in 0..5 {
        on_width(&mut app, snap, KeyCode::Down, 120);
    }
    let picked = input::key_command(&mut app, snap, input::key(KeyCode::Enter), 120, 30);
    let other = line_matching(&app, snap, 120, 30, |line| line.text.starts_with("Other "));
    assert_eq!(picked, Command::SelectProject(1));
    assert_eq!(
        input::click_command(&app, snap, other.x, other.y, 120, 30),
        picked
    );
    assert_eq!(
        on_width(&mut app, snap, KeyCode::Esc, 120),
        Command::Nothing
    );
    assert!(app.filter.is_none());
    assert!(app.stack.is_empty());
    assert!(
        compose::side_rows(&app, snap)
            .iter()
            .any(|(text, _)| text.starts_with("Horizon "))
    );

    app.stack.clear();
    app.filter = None;
    app.focus = Focus::Work;
    assert_eq!(on_key(&mut app, snap, KeyCode::Char('/')), Command::Nothing);
    assert!(app.stack.is_empty());
    assert_eq!(app.focus, Focus::Projects);
    assert_eq!(on_key(&mut app, snap, KeyCode::Esc), Command::Nothing);
    assert!(app.filter.is_none());
    assert!(app.stack.is_empty());

    app.focus = Focus::Work;
    on_key(&mut app, snap, KeyCode::Tab);
    assert_eq!(app.focus, Focus::Tabs);
    on_key(&mut app, snap, KeyCode::Tab);
    assert_eq!(app.focus, Focus::Overview);
    app.focus = Focus::Prompt;
    assert_eq!(on_key(&mut app, snap, KeyCode::Char('/')), Command::Nothing);
    assert_eq!(app.filter, None);
    assert_eq!(app.prompt_mut(&app.project_id(snap).unwrap()).draft, "/");

    app.focus = Focus::Overview;
    app.stack.clear();
    on_key(&mut app, snap, KeyCode::Char('g'));
    on_key(&mut app, snap, KeyCode::Char('3'));
    let copied = input::key_command(&mut app, snap, input::key(KeyCode::Char('y')), 160, 40);
    let path = horizon(snap).library[0].path.clone();
    assert_eq!(copied, Command::CopyPath(path.clone()));
    assert_eq!(on_key(&mut app, snap, KeyCode::Enter), Command::ReadLibrary);
    on_key(&mut app, snap, KeyCode::Down);
    on_key(&mut app, snap, KeyCode::Down);
    let row = line_matching(&app, snap, 160, 40, |line| {
        line.text.starts_with("Copy path ")
    });
    assert_eq!(
        input::key_command(&mut app, snap, input::key(KeyCode::Char('y')), 160, 40),
        copied
    );
    assert_eq!(
        input::click_command(&app, snap, row.x, row.y, 160, 40),
        copied
    );
    let outcome = run_follow(&ctx, &mut app, snap, copied);
    assert_eq!(outcome.status, "done");
    assert!(outcome.message.contains(&path), "{}", outcome.message);

    let note = fixture._root.path().join("horizon/library/t-0001/note.txt");
    std::fs::create_dir_all(note.parent().unwrap()).unwrap();
    std::fs::write(&note, "note").unwrap();
    let again = load::load(fixture._root.path(), fixture._config.path()).unwrap();
    let mut app = app_on(&again);
    app.focus = Focus::Overview;
    on_key(&mut app, &again, KeyCode::Char('g'));
    on_key(&mut app, &again, KeyCode::Char('1'));
    let card = horizon(&again);
    let index = compose::display_index(card, "t-0001").expect("t-0001");
    app.nav.get_mut(&card.id).unwrap().threads.selected = index;
    assert_eq!(on_key(&mut app, &again, KeyCode::Enter), Command::Nothing);
    for _ in 0..4 {
        on_key(&mut app, &again, KeyCode::Down);
    }
    let file_copy = input::key_command(&mut app, &again, input::key(KeyCode::Char('y')), 160, 40);
    assert_eq!(file_copy, Command::CopyPath(note.display().to_string()));
    let file_row = line_matching(&app, &again, 160, 40, |line| line.command == file_copy);
    assert_eq!(
        input::click_command(&app, &again, file_row.x, file_row.y, 160, 40),
        file_copy
    );
    let outcome = run_follow(&ctx, &mut app, &again, file_copy);
    assert_eq!(outcome.status, "done");
    assert!(outcome.message.contains("note.txt"), "{}", outcome.message);

    app.stack.clear();
    app.focus = Focus::Work;
    let preview = on_key(&mut app, &again, KeyCode::Char('S'));
    assert_eq!(preview, Command::SweepPreview);
    let outcome = run_follow(&ctx, &mut app, &again, preview);
    assert_eq!(outcome.status, "done");
    assert!(
        outcome.message.contains("nothing to clean"),
        "{}",
        outcome.message
    );
    assert!(!matches!(
        app.stack.last(),
        Some(state::Layer::Confirm { .. })
    ));

    let done = fixture._root.path().join("horizon/inbox/done/old.md");
    std::fs::create_dir_all(done.parent().unwrap()).unwrap();
    std::fs::write(&done, "old").unwrap();
    let file = std::fs::File::options().write(true).open(&done).unwrap();
    file.set_modified(
        std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 60 * 60),
    )
    .unwrap();
    let preview = on_key(&mut app, &again, KeyCode::Char('S'));
    let outcome = run_follow(&ctx, &mut app, &again, preview);
    assert_eq!(outcome.status, "confirm");
    assert!(
        outcome.message.contains("Remove all"),
        "{}",
        outcome.message
    );
    assert!(done.is_file());
    assert_eq!(
        on_key(&mut app, &again, KeyCode::Char('n')),
        Command::Nothing
    );
    assert!(done.is_file());
    assert!(!matches!(
        app.stack.last(),
        Some(state::Layer::Confirm { .. })
    ));
    let preview = on_key(&mut app, &again, KeyCode::Char('S'));
    run_follow(&ctx, &mut app, &again, preview);
    let yes = on_key(&mut app, &again, KeyCode::Char('y'));
    assert_eq!(yes, Command::SweepYes);
    let outcome = run_follow(&ctx, &mut app, &again, yes);
    assert_eq!(outcome.status, "done");
    assert!(!done.exists(), "{}", outcome.message);

    app.stack.clear();
    app.select_slug(&again, "horizon");
    on_key(&mut app, &again, KeyCode::Char('m'));
    for _ in 0..12 {
        on_key(&mut app, &again, KeyCode::Down);
    }
    assert_eq!(on_key(&mut app, &again, KeyCode::Enter), Command::Nothing);
    for ch in ['h', 'o', 'r', 'i', 'z', 'o', 'n', '-', 'x'] {
        on_key(&mut app, &again, KeyCode::Char(ch));
    }
    assert_eq!(on_key(&mut app, &again, KeyCode::Enter), Command::Nothing);
    let rename = on_key(&mut app, &again, KeyCode::Char('y'));
    assert_eq!(rename, Command::Rename);
    let outcome = run_follow(&ctx, &mut app, &again, rename);
    assert_eq!(outcome.status, "failed");
    assert!(
        outcome.message.contains("not resolved"),
        "{}",
        outcome.message
    );
    assert!(fixture._root.path().join("horizon").is_dir());
    assert!(!fixture._root.path().join("horizon-x").exists());

    crate::lifecycle::set_status(&ctx, "other", crate::project::Status::Archived).unwrap();
    assert_eq!(
        crate::project::Project::load(fixture._root.path(), "other")
            .unwrap()
            .status(),
        crate::project::Status::Archived
    );
    app.stack.clear();
    app.select_slug(&again, "other");
    on_key(&mut app, &again, KeyCode::Char('m'));
    on_key(&mut app, &again, KeyCode::Up);
    assert_eq!(
        input::key_command(&mut app, &again, input::key(KeyCode::Enter), 160, 40),
        Command::Unarchive
    );
    assert_eq!(on_key(&mut app, &again, KeyCode::Enter), Command::Nothing);
    assert_eq!(
        on_key(&mut app, &again, KeyCode::Char('n')),
        Command::Nothing
    );
    assert_eq!(
        crate::project::Project::load(fixture._root.path(), "other")
            .unwrap()
            .status(),
        crate::project::Status::Archived
    );
    assert_eq!(on_key(&mut app, &again, KeyCode::Enter), Command::Nothing);
    let unarchive = on_key(&mut app, &again, KeyCode::Char('y'));
    assert_eq!(unarchive, Command::Unarchive);
    let outcome = run_follow(&ctx, &mut app, &again, unarchive);
    assert_eq!(outcome.status, "done", "{}", outcome.message);
    assert_eq!(
        crate::project::Project::load(fixture._root.path(), "other")
            .unwrap()
            .status(),
        crate::project::Status::Active
    );

    app.stack.clear();
    on_key(&mut app, &again, KeyCode::Char('m'));
    on_key(&mut app, &again, KeyCode::Down);
    assert_eq!(on_key(&mut app, &again, KeyCode::Enter), Command::Nothing);
    for ch in ['o', 't', 'h', 'e', 'r', '-', 't', 'w', 'o'] {
        on_key(&mut app, &again, KeyCode::Char(ch));
    }
    assert_eq!(on_key(&mut app, &again, KeyCode::Enter), Command::Nothing);
    let rename = on_key(&mut app, &again, KeyCode::Char('y'));
    assert_eq!(rename, Command::Rename);
    let outcome = run_follow(&ctx, &mut app, &again, rename);
    assert_eq!(outcome.status, "done", "{}", outcome.message);
    assert!(fixture._root.path().join("other-two").is_dir());
    assert!(!fixture._root.path().join("other").exists());
}

fn reload(fixture: &Fixture) -> Snapshot {
    load::load(fixture._root.path(), fixture._config.path()).unwrap()
}

fn offers_open(card: &load::Card) -> bool {
    state::coordinator_actions(Some(card))
        .iter()
        .any(|(_, command)| *command == Command::OpenConversation)
}

#[test]
fn contract_open_coordinator_requires_the_live_primary() {
    let fixture = fixture();
    let project = crate::project::Project::load(fixture._root.path(), "horizon").unwrap();
    let socket = fixture._root.path().join("live.sock");
    std::fs::write(&socket, b"").unwrap();
    project
        .update_coordinator(|record| {
            record.socket = socket.display().to_string();
            record.pane_id = "w1:p1".into();
            record.agent_name = "hpc-horizon".into();
            record.profile = "claude".into();
        })
        .unwrap();

    let snap = reload(&fixture);
    let card = horizon(&snap);
    assert!(
        card.coordinator.contains("unavailable"),
        "{}",
        card.coordinator
    );
    assert!(!offers_open(card), "{}", card.coordinator);
    let text = frame(&app_on(&snap), &snap, 120, 30);
    assert!(text.contains("Inspect coordinator"), "{text}");
    assert!(!text.contains("Open coordinator in Herdr"), "{text}");

    let set = |status: &str| {
        crate::coordinator::save_live(
            &project,
            &[crate::coordinator::LivePane {
                pane_id: "w1:p1".into(),
                agent_status: status.into(),
                ..crate::coordinator::LivePane::default()
            }],
        )
        .unwrap();
    };
    let expect = |status: &str, state: state::CoordinatorState, word: &str| {
        set(status);
        let snap = reload(&fixture);
        let card = horizon(&snap);
        assert_eq!(
            state::coordinator_state(&card.coordinator),
            state,
            "{}",
            card.coordinator
        );
        assert!(card.coordinator.contains(word), "{}", card.coordinator);
        assert!(offers_open(card), "{}", card.coordinator);
        let text = frame(&app_on(&snap), &snap, 120, 30);
        assert!(text.contains("Open coordinator in Herdr"), "{text}");
        assert!(text.contains(word), "{text}");
        assert!(!text.contains("Focus stays closed"), "{text}");
    };
    expect("working", state::CoordinatorState::Working, "working");
    expect("blocked", state::CoordinatorState::Blocked, "blocked");
    expect("starting", state::CoordinatorState::Starting, "starting");
    expect(
        "not-a-ready-state",
        state::CoordinatorState::Unknown,
        "unknown",
    );

    set("idle");
    let snap = reload(&fixture);
    let card = horizon(&snap);
    assert_eq!(
        state::coordinator_state(&card.coordinator),
        state::CoordinatorState::Recorded,
        "{}",
        card.coordinator
    );
    assert!(
        card.coordinator.contains("status idle"),
        "{}",
        card.coordinator
    );
    assert!(offers_open(card));
    let text = frame(&app_on(&snap), &snap, 120, 30);
    assert!(text.contains("Open coordinator in Herdr"), "{text}");

    let (store, row) = project.open_row().unwrap();
    let session = store.primary_session(&row.id).unwrap().unwrap();
    store.mark_session_stale(&session.id).unwrap();
    let snap = reload(&fixture);
    let card = horizon(&snap);
    assert!(
        card.coordinator.contains("stale: session"),
        "{}",
        card.coordinator
    );
    assert!(!offers_open(card), "{}", card.coordinator);
    let text = frame(&app_on(&snap), &snap, 120, 30);
    assert!(text.contains("Inspect stale coordinator"), "{text}");
    assert!(!text.contains("Open coordinator in Herdr"), "{text}");
}

#[test]
fn contract_o_opens_the_selected_attention_thread() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let mut app = app_on(snap);
    app.focus = Focus::Overview;
    let card = horizon(snap);
    let id = app.project_id(snap).unwrap();
    app.nav_mut(&id);
    let failed = card
        .threads
        .iter()
        .find(|thread| thread.pane_id == "w2:p3")
        .unwrap();
    assert_ne!(
        press(&mut app, snap, KeyCode::Char('o')),
        Command::OpenWorker
    );

    let need = &snap.needs[0];
    assert_ne!(need.thread_id, failed.id);
    let mut found = false;
    for _ in 0..40 {
        if compose::open_worker_thread(&app, snap).is_some_and(|thread| thread.id == need.thread_id)
        {
            found = true;
            break;
        }
        state::apply(&mut app, Command::Move(1), snap, 160);
    }
    assert!(found, "attention row {} was not selected", need.thread_id);
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('o')),
        Command::OpenWorker
    );
    let built = crate::screen::jobs::build(&app, snap, &Command::OpenWorker, Some("hp")).unwrap();
    assert_eq!(built.target_id, need.thread_id);
    assert_ne!(built.pane, failed.pane_id);
    assert_ne!(built.target_id, failed.id);

    state::apply(&mut app, Command::SelectTab(0), snap, 160);
    let tab = card
        .threads
        .iter()
        .find(|thread| thread.kind == "tab")
        .unwrap();
    let index = compose::display_index(card, &tab.id).unwrap();
    let id = app.project_id(snap).unwrap();
    app.nav_mut(&id).threads.set(index, card.threads.len(), 8);
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('o')),
        Command::OpenWorker
    );
    let built = crate::screen::jobs::build(&app, snap, &Command::OpenWorker, Some("hp")).unwrap();
    assert_eq!(built.target_id, tab.id);
    assert_ne!(built.target_id, failed.id);

    let start = card.threads.len();
    for _ in 0..start + 2 {
        if app
            .nav
            .get(&id)
            .is_some_and(|nav| nav.threads.selected == start)
        {
            break;
        }
        state::apply(&mut app, Command::Move(1), snap, 160);
    }
    assert_eq!(app.nav.get(&id).unwrap().threads.selected, start);
    assert!(compose::open_worker_thread(&app, snap).is_none());
    assert_ne!(
        press(&mut app, snap, KeyCode::Char('o')),
        Command::OpenWorker
    );
    let built = crate::screen::jobs::build(&app, snap, &Command::OpenWorker, Some("hp")).unwrap();
    assert!(built.pane.is_empty(), "{}", built.pane);
    assert!(built.target_id.is_empty(), "{}", built.target_id);

    let previous = compose::thread_by_selection(card, start - 1).unwrap();
    assert!(!previous.id.is_empty());
    assert_eq!(compose::detail_rows(&app, snap)[0].0, "no thread");
    assert_eq!(
        press(&mut app, snap, KeyCode::Char('t')),
        Command::StartThreadForm
    );
    let keys = [
        (KeyCode::Char('r'), Command::Restart),
        (KeyCode::Char('s'), Command::Stop),
        (KeyCode::Char('a'), Command::Ack),
        (KeyCode::Char('x'), Command::Resolve),
        (KeyCode::Char('1'), Command::NextLine(0)),
    ];
    for (code, command) in keys {
        assert_eq!(press(&mut app, snap, code), Command::Nothing);
        let built = crate::screen::jobs::build(&app, snap, &command, Some("hp"));
        assert!(
            built.is_none(),
            "start row copied {} pane {} into {} pane {}",
            previous.id,
            previous.pane_id,
            built
                .as_ref()
                .map(|row| row.target_id.as_str())
                .unwrap_or(""),
            built.as_ref().map(|row| row.pane.as_str()).unwrap_or("")
        );
    }
}

#[test]
fn contract_thread_rows_show_harness_workspace_and_location() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let card = horizon(snap);
    assert!(
        card.threads
            .iter()
            .any(|thread| thread.kind == "tab" && thread.harness == "codex")
    );
    assert!(card.threads.iter().any(|thread| thread.kind == "checkout"));
    assert!(card.threads.iter().any(|thread| thread.machine == "box-a"));
    assert!(
        card.threads
            .iter()
            .any(|thread| thread.worktree.ends_with("wt-one"))
    );
    assert!(
        card.threads
            .iter()
            .any(|thread| thread.worktree.ends_with("wt-two"))
    );
    let mut app = app_on(snap);
    state::apply(&mut app, Command::SelectTab(0), snap, 160);
    let text = frame(&app, snap, 160, 40);
    assert!(text.contains(" codex "), "{text}");
    assert!(text.contains(" tab "), "{text}");
    assert!(text.contains(" checkout "), "{text}");
    assert!(text.contains(" box-a "), "{text}");
    assert!(text.contains("wt-one"), "{text}");
    assert!(text.contains("wt-two"), "{text}");
}

#[test]
fn contract_failed_import_shows_the_diagnostic() {
    let fixture = fixture();
    let snap = &fixture.snap;
    let failed = snap
        .failed
        .iter()
        .find(|failed| failed.message.contains("does not parse"))
        .unwrap();
    let mut app = app_on(snap);
    app.focus = Focus::Projects;
    let row = line_matching(&app, snap, 120, 30, |line| {
        line.text.contains("does not parse")
    });
    assert!(
        matches!(row.command, Command::ShowImport { .. }),
        "{:?}",
        row.command
    );
    assert_eq!(
        input::click_command(&app, snap, row.x, row.y, 120, 30),
        row.command
    );
    let index = compose::side_rows(&app, snap)
        .iter()
        .position(|(text, _)| text.contains("does not parse"))
        .unwrap();
    app.side_list.selected = index;
    let keyed = input::key_command(&mut app, snap, input::key(KeyCode::Enter), 120, 30);
    assert_eq!(keyed, row.command);
    state::apply(&mut app, keyed, snap, 120);
    let id = app.project_id(snap).unwrap();
    assert!(app.notices[&id].contains("does not parse"));
    assert!(app.notices[&id].contains(&failed.path));
}
