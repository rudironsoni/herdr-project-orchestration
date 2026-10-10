use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::coordinator;
use crate::paths::Ctx;
use crate::screen::draw;
use crate::screen::input;
use crate::screen::jobs::{self, Lane};
use crate::screen::load;
use crate::screen::state::{App, Command};

pub fn run(ctx: &Ctx, scope: Option<String>) -> Result<()> {
    if !io::stdout().is_terminal() {
        bail!("popup needs a terminal");
    }
    let mut snap = load::load(&ctx.root, &ctx.config_dir)?;
    let mut app = App::default();
    if let Some(slug) = &scope {
        app.select_slug(&snap, slug);
    }
    let _restorer = Restorer::enter()?;
    arm_stop();
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut lane = Lane::new();
    let prefix = coordinator::current_prefix(&ctx.root).ok();
    loop {
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        terminal.draw(|frame| draw::draw(frame, &app, &snap))?;
        let area = terminal.size()?;
        if event::poll(Duration::from_millis(50))? {
            let follow = match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    input::on_key(&mut app, &snap, key, area.width, area.height)
                }
                Event::Mouse(mouse) => {
                    mouse_command(&mut app, &snap, mouse, area.width, area.height)
                }
                Event::Resize(_, _) => {
                    let _ = terminal.clear();
                    terminal.draw(|frame| draw::draw(frame, &app, &snap))?;
                    Command::Nothing
                }
                _ => Command::Nothing,
            };
            if follow == Command::Quit {
                break;
            }
            jobs::launch(&mut lane, ctx, &mut app, &snap, &follow, prefix.as_deref());
        }
        let mut reload = false;
        let mut created = None;
        while let Some(outcome) = lane.try_recv() {
            if outcome.intent.starts_with("new:") && outcome.status != "failed" {
                created = Some(outcome.slug.clone());
            }
            jobs::apply_outcome(&mut app, &outcome);
            reload = true;
        }
        if reload {
            snap = load::load(&ctx.root, &ctx.config_dir)?;
            if let Some(slug) = created {
                app.select_slug(&snap, &slug);
            }
        }
    }
    if let Some(focus) = app.pending_focus.take() {
        jobs::retry_focus(
            &ctx.env.herdr_bin(),
            &focus.socket,
            &focus.machine,
            &focus.pane,
        );
    }
    Ok(())
}

fn mouse_command(
    app: &mut App,
    snap: &crate::screen::load::Snapshot,
    mouse: ratatui::crossterm::event::MouseEvent,
    width: u16,
    height: u16,
) -> Command {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let command = input::click_command(app, snap, mouse.column, mouse.row, width, height);
            crate::screen::state::apply(app, command, snap, width)
        }
        MouseEventKind::ScrollUp => {
            crate::screen::state::apply(app, Command::Move(-1), snap, width)
        }
        MouseEventKind::ScrollDown => {
            crate::screen::state::apply(app, Command::Move(1), snap, width)
        }
        _ => Command::Nothing,
    }
}

struct Restorer;

impl Restorer {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, enable_mouse_capture())?;
        Ok(Restorer)
    }
}

impl Drop for Restorer {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, disable_mouse_capture());
    }
}

fn enable_mouse_capture() -> ratatui::crossterm::event::EnableMouseCapture {
    ratatui::crossterm::event::EnableMouseCapture
}

fn disable_mouse_capture() -> ratatui::crossterm::event::DisableMouseCapture {
    ratatui::crossterm::event::DisableMouseCapture
}

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

fn arm_stop() {
    let handler = on_stop as *const () as usize;
    unsafe {
        crate::runner::set_signal(1, handler);
        crate::runner::set_signal(2, handler);
        crate::runner::set_signal(15, handler);
    }
}
