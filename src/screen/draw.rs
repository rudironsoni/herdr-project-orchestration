#[cfg(test)]
use ratatui::Terminal;
#[cfg(test)]
use ratatui::backend::TestBackend;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Widget};

use crate::screen::compose::{self, Placed};
use crate::screen::load::Snapshot;
use crate::screen::state::{App, Command};

pub fn draw(frame: &mut ratatui::Frame, app: &App, snap: &Snapshot) {
    let area = frame.area();
    let lines = compose::plan(app, snap, area.width, area.height);
    let buf = frame.buffer_mut();
    if compose::framed(area.width, area.height) {
        let dim = Style::default().fg(Color::DarkGray);
        let mut block = Block::bordered()
            .border_style(dim)
            .title_top(title_line(&compose::frame_title(app, snap)));
        let status = compose::frame_status(app, snap, area.width);
        if !status.is_empty() {
            block = block.title_top(Line::styled(status, dim).alignment(Alignment::Right));
        }
        block = block.title_bottom(footer_line(&compose::frame_footer(app, area.width)));
        block.render(area, buf);
    }
    for line in &lines {
        paint(buf, line, area.width);
    }
    join_panels(buf, &lines);
}

fn title_line(title: &str) -> Line<'static> {
    let brand = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let name = Style::default().add_modifier(Modifier::BOLD);
    match title.split_once("PROJECTS") {
        Some((head, tail)) => Line::from(vec![
            Span::styled(head.to_string(), brand),
            Span::styled("PROJECTS".to_string(), name),
            Span::styled(tail.to_string(), name),
        ]),
        None => Line::styled(title.to_string(), name),
    }
}

fn footer_line(text: &str) -> Line<'static> {
    const KEYS: &[&str] = &["Enter", "Tab", "Esc", "P", "o", "/", "g1-6", "Up", "Down"];
    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(" ", dim)];
    let mut rest = text;
    while !rest.is_empty() {
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..word_end];
        let style = if KEYS.contains(&word) { key } else { dim };
        spans.push(Span::styled(word.to_string(), style));
        rest = &rest[word_end..];
        let gap_end = rest
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(rest.len());
        if gap_end > 0 {
            spans.push(Span::styled(rest[..gap_end].to_string(), dim));
            rest = &rest[gap_end..];
        }
    }
    Line::from(spans)
}

fn paint(buf: &mut ratatui::buffer::Buffer, line: &Placed, screen_w: u16) {
    if line.y >= buf.area.height || line.x >= screen_w || line.width == 0 {
        return;
    }
    let width = line.width.min(screen_w.saturating_sub(line.x));
    let selected = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    if line.selected {
        buf.set_style(
            Rect {
                x: line.x,
                y: line.y,
                width,
                height: 1,
            },
            selected,
        );
    }
    if line.text == "│" {
        buf.set_string(line.x, line.y, &line.text, dim);
        return;
    }
    if is_label(&line.text) && !line.selected {
        paint_label(buf, line, width, dim);
        return;
    }
    let (x, width) = inset(line, width);
    let base = if line.selected {
        selected
    } else if is_counts(&line.text) {
        buf.set_stringn(x, line.y, &line.text, width as usize, dim);
        return;
    } else if is_alert(&line.text) {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if is_current_chip(&line.text) {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else if is_button(&line.command) {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if line.text.starts_with('[') {
        dim
    } else if line.text.starts_with('>') {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    paint_spans(buf, x, line.y, width, &line.text, base);
}

fn inset(line: &Placed, width: u16) -> (u16, u16) {
    let len = line.text.chars().count() as u16;
    if width > 1 && !line.text.starts_with('[') && len.saturating_add(1) < width {
        (line.x.saturating_add(1), width.saturating_sub(1))
    } else {
        (line.x, width)
    }
}

fn paint_label(buf: &mut ratatui::buffer::Buffer, line: &Placed, width: u16, dim: Style) {
    let bold = dim.add_modifier(Modifier::BOLD);
    let name = line.text.strip_prefix("SELECTED / ");
    let head = if name.is_some() {
        "SELECTED"
    } else {
        line.text.as_str()
    };
    let mut used = write_at(buf, line.x, line.y, width, 0, " ", dim);
    used = write_at(buf, line.x, line.y, width, used, head, bold);
    if let Some(name) = name {
        let gap = format!(" / {name}");
        used = write_at(
            buf,
            line.x,
            line.y,
            width,
            used,
            &gap,
            Style::default().add_modifier(Modifier::BOLD),
        );
    }
    if is_panel(&line.text) && used.saturating_add(1) < width {
        used = write_at(buf, line.x, line.y, width, used, " ", dim);
        let rule = "─".repeat((width - used) as usize);
        write_at(buf, line.x, line.y, width, used, &rule, dim);
    }
}

fn write_at(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    used: u16,
    text: &str,
    style: Style,
) -> u16 {
    if used >= width {
        return used;
    }
    let room = (width - used) as usize;
    buf.set_stringn(x.saturating_add(used), y, text, room, style);
    used.saturating_add(text.chars().count().min(room) as u16)
}

fn join_panels(buf: &mut ratatui::buffer::Buffer, lines: &[Placed]) {
    for line in lines {
        if line.selected || !is_panel(&line.text) {
            continue;
        }
        mark_junction(buf, line.x.saturating_sub(1), line.y, true);
        mark_junction(buf, line.x.saturating_add(line.width), line.y, false);
    }
}

fn mark_junction(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, left: bool) {
    let Some(cell) = buf.cell_mut((x, y)) else {
        return;
    };
    let symbol = cell.symbol().to_string();
    let Some(next) = junction(&symbol, left) else {
        return;
    };
    cell.set_symbol(next);
    cell.set_style(Style::default().fg(Color::DarkGray));
}

fn junction(symbol: &str, left: bool) -> Option<&'static str> {
    if left {
        match symbol {
            "│" | "├" | "┌" | "└" => Some("├"),
            "┤" | "┼" | "┐" | "┘" => Some("┼"),
            _ => None,
        }
    } else {
        match symbol {
            "│" | "┤" | "┐" | "┘" => Some("┤"),
            "├" | "┼" | "┌" | "└" => Some("┼"),
            _ => None,
        }
    }
}

fn paint_spans(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    base: Style,
) {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let mut col = 0u16;
    let mut token_start = true;
    while index < chars.len() && col < width {
        if token_start && let Some(len) = status_len(&chars[index..]) {
            let word: String = chars[index..index + len].iter().collect();
            let style = status_style(&word).patch(base);
            buf.set_string(x.saturating_add(col), y, &word, style);
            col = col.saturating_add(len as u16);
            index += len;
            token_start = false;
            continue;
        }
        let ch = chars[index];
        let style = if index == 0 && ch == '>' {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
                .patch(base)
        } else {
            base
        };
        buf.set_string(x.saturating_add(col), y, ch.to_string(), style);
        token_start = ch.is_whitespace();
        col = col.saturating_add(1);
        index += 1;
    }
}

fn is_panel(text: &str) -> bool {
    matches!(text, "PROJECTS" | "OVERVIEW") || text.starts_with("SELECTED / ") || text == "SELECTED"
}

fn is_label(text: &str) -> bool {
    matches!(
        text,
        "PROJECTS"
            | "GOAL"
            | "COORDINATOR"
            | "OVERVIEW"
            | "WORK"
            | "HEALTH"
            | "NEEDS ATTENTION"
            | "ACTIVE"
            | "RECENT"
    ) || text.starts_with("SELECTED / ")
        || text == "SELECTED"
}

fn is_counts(text: &str) -> bool {
    text.contains(" open ") && text.contains(" failed ") && text.contains(" resolved")
}

fn is_alert(text: &str) -> bool {
    text.contains("does not parse") || text.contains("unresolved")
}

fn is_current_chip(text: &str) -> bool {
    let Some(inner) = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    !inner.is_empty() && inner.chars().all(|ch| ch.is_ascii_uppercase())
}

fn is_button(command: &Command) -> bool {
    matches!(
        command,
        Command::InspectCoordinator
            | Command::OpenConversation
            | Command::ReadLibrary
            | Command::OpenMenu
            | Command::FocusPrompt
            | Command::StartCoordinator
            | Command::FinishSetup
            | Command::SubmitPrompt
            | Command::StartThreadForm
            | Command::SubmitThread
            | Command::SubmitNew
            | Command::Cancel
            | Command::ConfirmYes
    )
}

fn status_len(chars: &[char]) -> Option<usize> {
    const WORDS: &[&str] = &[
        "unavailable",
        "blocked",
        "missing",
        "working",
        "starting",
        "unknown",
        "stale",
        "failed",
    ];
    let text: String = chars.iter().collect();
    for word in WORDS {
        if text.starts_with(word) {
            let next = chars.get(word.len()).copied();
            if next.is_none_or(status_boundary) {
                return Some(word.len());
            }
        }
    }
    None
}

fn status_boundary(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '.' | ',' | ':' | ';' | '…')
}

fn status_style(word: &str) -> Style {
    match word {
        "working" => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        "starting" | "unknown" => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
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
