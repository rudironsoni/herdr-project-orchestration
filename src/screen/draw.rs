#[cfg(test)]
use ratatui::Terminal;
#[cfg(test)]
use ratatui::backend::TestBackend;
use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Widget};

use crate::screen::compose;
use crate::screen::load::Snapshot;
use crate::screen::state::App;

pub fn draw(frame: &mut ratatui::Frame, app: &App, snap: &Snapshot) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    if compose::framed(area.width, area.height) {
        let mut block = Block::bordered().title_top(compose::frame_title(app, snap));
        let status = compose::frame_status(app, snap, area.width);
        if !status.is_empty() {
            block = block.title_top(Line::from(status).alignment(Alignment::Right));
        }
        block = block.title_bottom(compose::frame_footer(app, area.width));
        block.render(area, buf);
    }
    let lines = compose::plan(app, snap, area.width, area.height);
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
