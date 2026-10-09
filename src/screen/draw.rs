#[cfg(test)]
use ratatui::Terminal;
#[cfg(test)]
use ratatui::backend::TestBackend;
use ratatui::style::{Modifier, Style};

use crate::screen::compose;
use crate::screen::load::Snapshot;
use crate::screen::state::App;

pub fn draw(frame: &mut ratatui::Frame, app: &App, snap: &Snapshot) {
    let area = frame.area();
    let lines = compose::plan(app, snap, area.width, area.height);
    let buf = frame.buffer_mut();
    for line in lines {
        if line.y >= area.height || line.x >= area.width {
            continue;
        }
        let style = if line.selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        buf.set_string(line.x, line.y, &line.text, style);
    }
}

#[cfg(test)]
pub fn frame_text(app: &App, snap: &Snapshot, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| draw(frame, app, snap)).expect("draw");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
