use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Frame,
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

/// Draw the content area for the active tab (placeholder for now).
fn draw_tab_content(frame: &mut Frame, app: &App, area: Rect) {
    let label = app.active_tab.title();
    let content = Paragraph::new(format!("  {label}  "))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", label)),
        );

    frame.render_widget(content, area);
}
