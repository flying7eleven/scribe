use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs},
    Frame,
};

use super::tabs::events::{self, DetailMode};
use crate::format::{format_timestamp, truncate_path};

use super::app::{App, Tab};
use super::help;

/// Draw the entire TUI frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(frame.area());

    draw_tab_bar(frame, app, chunks[0]);
    draw_tab_content(frame, app, chunks[1]);

    if app.show_help {
        help::draw_help(frame);
    }
}

/// Draw the tab bar at the top of the screen.
fn draw_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, tab)| Line::from(Span::raw(format!(" {} {} ", i + 1, tab.title()))))
        .collect();

    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(" scribe tui "))
        .select(app.active_tab.index())
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw("|"));

    frame.render_widget(tabs, area);
}

/// Draw the content area for the active tab.
fn draw_tab_content(frame: &mut Frame, app: &App, area: Rect) {
    match app.active_tab {
        Tab::Sessions => draw_sessions_tab(frame, app, area),
        Tab::Events => draw_events_tab(frame, app, area),
        _ => draw_placeholder(frame, app.active_tab.title(), area),
    }
}

/// Draw a placeholder for tabs not yet implemented.
fn draw_placeholder(frame: &mut Frame, label: &str, area: Rect) {
    let content = Paragraph::new(format!("  {label}  "))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", label)),
        );
    frame.render_widget(content, area);
}

/// Draw the Sessions tab with a table of sessions.
fn draw_sessions_tab(frame: &mut Frame, app: &App, area: Rect) {
    let sessions = &app.sessions;

    if sessions.sessions.is_empty() {
        let empty = Paragraph::new("(empty)")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Sessions "));
        frame.render_widget(empty, area);
        return;
    }

    // Split area into table + status line
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let header = Row::new(vec![
        Cell::from("Session ID"),
        Cell::from("First Seen"),
        Cell::from("Last Seen"),
        Cell::from("CWD"),
        Cell::from("Events"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = sessions
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let style = if i == sessions.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let sid = if s.session_id.len() > 12 {
                &s.session_id[..12]
            } else {
                &s.session_id
            };

            Row::new(vec![
                Cell::from(sid.to_string()),
                Cell::from(format_timestamp(&s.first_seen)),
                Cell::from(format_timestamp(&s.last_seen)),
                Cell::from(
                    s.cwd
                        .as_deref()
                        .map(|p| truncate_path(p, 30))
                        .unwrap_or_default(),
                ),
                Cell::from(s.event_count.to_string()),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(21),
            Constraint::Length(21),
            Constraint::Min(20),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Sessions "));

    frame.render_widget(table, chunks[0]);

    // Status line
    let count = sessions.sessions.len();
    let status = Paragraph::new(format!(
        " {} session{} | ↑↓/jk navigate | Enter drill-down | q quit",
        count,
        if count == 1 { "" } else { "s" }
    ))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[1]);
}

/// Draw the Events tab with a table and optional inline detail.
fn draw_events_tab(frame: &mut Frame, app: &App, area: Rect) {
    let ev = &app.events;

    if ev.events.is_empty() {
        let msg = if ev.session_filter.is_some() {
            "(no events for this session)"
        } else {
            "(empty)"
        };
        let empty = Paragraph::new(msg)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Events "));
        frame.render_widget(empty, area);
        return;
    }

    // Split: table area + detail area (if expanded) + status line
    let has_detail = ev.expanded.is_some();
    let constraints = if has_detail {
        vec![
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Length(1),
        ]
    } else {
        vec![Constraint::Min(0), Constraint::Length(1)]
    };
    let chunks = Layout::vertical(constraints).split(area);

    // Event table
    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("Timestamp"),
        Cell::from("Event Type"),
        Cell::from("Tool"),
        Cell::from("Session"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = ev
        .events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let style = if i == ev.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let sid = if e.session_id.len() > 8 {
                &e.session_id[..8]
            } else {
                &e.session_id
            };

            Row::new(vec![
                Cell::from(e.id.to_string()),
                Cell::from(format_timestamp(&e.timestamp)),
                Cell::from(e.event_type.clone()),
                Cell::from(e.tool_name.clone().unwrap_or_default()),
                Cell::from(sid.to_string()),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(21),
            Constraint::Length(22),
            Constraint::Length(16),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Events "));

    frame.render_widget(table, chunks[0]);

    // Detail panel (if expanded)
    if has_detail {
        if let Some(idx) = ev.expanded {
            if let Some(event) = ev.events.get(idx) {
                let detail_lines = match ev.detail_mode {
                    DetailMode::Structured => events::format_structured_detail(event),
                    DetailMode::RawJson => events::format_raw_json(event),
                };

                let mode_label = match ev.detail_mode {
                    DetailMode::Structured => "Structured",
                    DetailMode::RawJson => "Raw JSON",
                };

                let text: Vec<Line> = detail_lines.into_iter().map(Line::from).collect();
                let detail = Paragraph::new(text).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" Detail [{mode_label}] — Tab to toggle ")),
                );
                frame.render_widget(detail, chunks[1]);
            }
        }
    }

    // Status line
    let status_chunk = if has_detail { chunks[2] } else { chunks[1] };

    let count = ev.events.len();
    let filter_hint = if let Some(ref sid) = ev.session_filter {
        let short = if sid.len() > 12 { &sid[..12] } else { sid };
        format!(" | Filtered: session {short}... (Esc to clear)")
    } else {
        String::new()
    };

    let status = Paragraph::new(format!(
        " {} event{} | Enter expand | Tab toggle view | Esc collapse{filter_hint}",
        count,
        if count == 1 { "" } else { "s" }
    ))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, status_chunk);
}
