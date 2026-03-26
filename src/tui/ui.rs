use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs},
    Frame,
};

use super::filter::{filter_events, filter_sessions};
use super::tabs::events::{self, DetailMode};
use crate::format::{
    format_count, format_date_label, format_duration, format_size, format_timestamp, histogram_bar,
    truncate_path,
};

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
        Tab::Stats => draw_stats_tab(frame, app, area),
        Tab::Live => draw_live_tab(frame, app, area),
    }
}

/// Draw the Sessions tab with a table of sessions.
fn draw_sessions_tab(frame: &mut Frame, app: &App, area: Rect) {
    let sessions = &app.sessions;
    let filtered = filter_sessions(&app.filter, &sessions.sessions);

    if sessions.sessions.is_empty() {
        let empty = Paragraph::new("(empty)")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Sessions "));
        frame.render_widget(empty, area);
        return;
    }

    // Split area into table + status/filter line
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

    let rows: Vec<Row> = filtered
        .iter()
        .map(|&idx| {
            let s = &sessions.sessions[idx];
            let style = if idx == sessions.selected {
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

    // Status / filter line
    if app.filter.active {
        let filter_line = format!(
            "/ {}█  ({} of {} sessions)",
            app.filter.input,
            filtered.len(),
            sessions.sessions.len()
        );
        let status = Paragraph::new(filter_line).style(Style::default().fg(Color::Cyan));
        frame.render_widget(status, chunks[1]);
    } else if !app.filter.input.is_empty() {
        let status = Paragraph::new(format!(
            " {} of {} sessions (/ to filter, Esc to clear)",
            filtered.len(),
            sessions.sessions.len()
        ))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[1]);
    } else {
        let count = sessions.sessions.len();
        let status = Paragraph::new(format!(
            " {} session{} | / filter | Enter drill-down | q quit",
            count,
            if count == 1 { "" } else { "s" }
        ))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[1]);
    }
}

/// Draw the Events tab with a table and optional inline detail.
fn draw_events_tab(frame: &mut Frame, app: &App, area: Rect) {
    let ev = &app.events;
    let filtered = filter_events(&app.filter, &ev.events);

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

    let rows: Vec<Row> = filtered
        .iter()
        .map(|&idx| {
            let e = &ev.events[idx];
            let style = if idx == ev.selected {
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
                        .title(format!(" Detail [{mode_label}] \u{2014} Tab to toggle ")),
                );
                frame.render_widget(detail, chunks[1]);
            }
        }
    }

    // Status line
    let status_chunk = if has_detail { chunks[2] } else { chunks[1] };

    if app.filter.active {
        let filter_line = format!(
            "/ {}\u{2588}  ({} of {} events)",
            app.filter.input,
            filtered.len(),
            ev.events.len()
        );
        let status = Paragraph::new(filter_line).style(Style::default().fg(Color::Cyan));
        frame.render_widget(status, status_chunk);
    } else {
        let session_hint = if let Some(ref sid) = ev.session_filter {
            let short = if sid.len() > 12 { &sid[..12] } else { sid };
            format!(" | session {short}...")
        } else {
            String::new()
        };
        let filter_count = if !app.filter.input.is_empty() {
            format!(" ({} of {})", filtered.len(), ev.events.len())
        } else {
            String::new()
        };
        let status = Paragraph::new(format!(
            " {} event{}{filter_count} | / filter | Enter expand{session_hint}",
            filtered.len(),
            if filtered.len() == 1 { "" } else { "s" }
        ))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, status_chunk);
    }
}

/// Draw the Stats tab as a scrollable text dashboard.
fn draw_stats_tab(frame: &mut Frame, app: &App, area: Rect) {
    let st = &app.stats;

    let Some(ref stats) = st.stats else {
        let loading = Paragraph::new("Loading...")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Stats "));
        frame.render_widget(loading, area);
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // Header
    lines.push(Line::from(format!("Database:    {}", st.db_path)));
    lines.push(Line::from(format!(
        "Size:        {}",
        format_size(st.db_size)
    )));
    lines.push(Line::from(format!(
        "Events:      {}",
        format_count(stats.event_count)
    )));
    lines.push(Line::from(format!(
        "Sessions:    {}",
        format_count(stats.session_count)
    )));
    if let Some(avg) = st.avg_duration {
        lines.push(Line::from(format!(
            "Avg duration:  {}",
            format_duration(avg)
        )));
    }
    let oldest = stats
        .oldest_event
        .as_deref()
        .map(format_timestamp)
        .unwrap_or_else(|| "\u{2014}".to_string());
    let newest = stats
        .newest_event
        .as_deref()
        .map(format_timestamp)
        .unwrap_or_else(|| "\u{2014}".to_string());
    lines.push(Line::from(format!("Oldest:      {oldest}")));
    lines.push(Line::from(format!("Newest:      {newest}")));

    if stats.event_count == 0 {
        let content = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Stats "))
            .scroll((st.scroll_offset, 0));
        frame.render_widget(content, area);
        return;
    }

    // Top tools
    if !st.tools.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Top tools:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (i, tool) in st.tools.iter().enumerate() {
            lines.push(Line::from(format!(
                "  {:>2}. {:<20} {}",
                i + 1,
                tool.tool_name,
                format_count(tool.count)
            )));
        }
    }

    // Event types
    if !st.event_types.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Event types:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for et in &st.event_types {
            lines.push(Line::from(format!(
                "  {:<24} {}",
                et.event_type,
                format_count(et.count)
            )));
        }
    }

    // Errors
    if let Some(ref errors) = st.errors {
        lines.push(Line::from(""));
        if errors.post_tool_use_failure_count == 0 && errors.stop_failure_count == 0 {
            lines.push(Line::from("Errors:              none"));
        } else {
            lines.push(Line::from(Span::styled(
                "Errors:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            if errors.post_tool_use_failure_count > 0 {
                lines.push(Line::from(format!(
                    "  {:<24} {}",
                    "PostToolUseFailure",
                    format_count(errors.post_tool_use_failure_count)
                )));
            }
            if errors.stop_failure_count > 0 {
                lines.push(Line::from(format!(
                    "  {:<24} {}",
                    "StopFailure",
                    format_count(errors.stop_failure_count)
                )));
                for sf in &errors.stop_failure_types {
                    lines.push(Line::from(format!(
                        "    {:<22} {}",
                        sf.error_type,
                        format_count(sf.count)
                    )));
                }
            }
        }
    }

    // Top directories
    if !st.dirs.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Top directories:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (i, dir) in st.dirs.iter().enumerate() {
            let path = truncate_path(&dir.cwd, 40);
            lines.push(Line::from(format!(
                "  {:>2}. {:<40} {}",
                i + 1,
                path,
                format_count(dir.count)
            )));
        }
    }

    // Activity histogram
    if !st.activity.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Activity:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        let max_count = st.activity.iter().map(|(_, c)| *c).max().unwrap_or(0);
        for (date_str, count) in &st.activity {
            let label = format_date_label(date_str);
            let bar = histogram_bar(*count, max_count, 30);
            lines.push(Line::from(format!(
                "  {label}  {:<30} {}",
                bar,
                format_count(*count)
            )));
        }
    }

    let content = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Stats "))
        .scroll((st.scroll_offset, 0));
    frame.render_widget(content, area);
}

/// Draw the Live tab with stats summary and scrolling event feed.
fn draw_live_tab(frame: &mut Frame, app: &App, area: Rect) {
    let live = &app.live;

    // Split into top stats pane and bottom feed pane
    let chunks = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    // Top pane: stats summary
    let stats_text = if let Some(ref stats) = live.stats_snapshot {
        format!(
            "  Events: {}    Sessions: {}    Rate: {:.1}/min    Uptime: {}",
            format_count(stats.event_count),
            format_count(stats.session_count),
            live.events_per_minute,
            live.uptime()
        )
    } else {
        "  Waiting for data...".to_string()
    };

    let stats_block = Paragraph::new(stats_text)
        .block(Block::default().borders(Borders::ALL).title(" Live Stats "));
    frame.render_widget(stats_block, chunks[0]);

    // Bottom pane: event feed
    if live.feed.is_empty() {
        let empty = Paragraph::new("  Waiting for events...")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Event Feed "));
        frame.render_widget(empty, chunks[1]);
    } else {
        let header = Row::new(vec![
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

        // Calculate visible window based on scroll position
        let visible_height = chunks[1].height.saturating_sub(3) as usize; // borders + header
        let total = live.feed.len();
        let start = if live.auto_scroll {
            total.saturating_sub(visible_height)
        } else {
            live.feed_scroll.min(total.saturating_sub(visible_height))
        };
        let end = (start + visible_height).min(total);

        let rows: Vec<Row> = live
            .feed
            .iter()
            .skip(start)
            .take(end - start)
            .map(|e| {
                let sid = if e.session_id.len() > 8 {
                    &e.session_id[..8]
                } else {
                    &e.session_id
                };
                Row::new(vec![
                    Cell::from(format_timestamp(&e.timestamp)),
                    Cell::from(e.event_type.clone()),
                    Cell::from(e.tool_name.clone().unwrap_or_default()),
                    Cell::from(sid.to_string()),
                ])
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(21),
                Constraint::Length(22),
                Constraint::Length(16),
                Constraint::Min(10),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Event Feed "));
        frame.render_widget(table, chunks[1]);
    }

    // Status line
    let pause_hint = if !live.auto_scroll {
        " (paused \u{2014} press G to resume)"
    } else {
        ""
    };
    let status = Paragraph::new(format!(" {} events in feed{pause_hint}", live.feed_len()))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, chunks[2]);
}
