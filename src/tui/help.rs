use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Draw the help overlay as a centered popup.
pub fn draw_help(frame: &mut Frame) {
    let area = centered_rect(60, 70, frame.area());

    let help_text = vec![
        Line::from(Span::styled(
            "Keybindings",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  1-5       ", Style::default().fg(Color::Cyan)),
            Span::raw("Switch to tab"),
        ]),
        Line::from(vec![
            Span::styled("  Tab       ", Style::default().fg(Color::Cyan)),
            Span::raw("Next tab"),
        ]),
        Line::from(vec![
            Span::styled("  Shift-Tab ", Style::default().fg(Color::Cyan)),
            Span::raw("Previous tab"),
        ]),
        Line::from(vec![
            Span::styled("  j/k       ", Style::default().fg(Color::Cyan)),
            Span::raw("Move down/up"),
        ]),
        Line::from(vec![
            Span::styled("  ↑/↓       ", Style::default().fg(Color::Cyan)),
            Span::raw("Move up/down"),
        ]),
        Line::from(vec![
            Span::styled("  g/G       ", Style::default().fg(Color::Cyan)),
            Span::raw("Jump to top/bottom"),
        ]),
        Line::from(vec![
            Span::styled("  Enter     ", Style::default().fg(Color::Cyan)),
            Span::raw("Select / expand"),
        ]),
        Line::from(vec![
            Span::styled("  /         ", Style::default().fg(Color::Cyan)),
            Span::raw("Filter (Sessions, Events)"),
        ]),
        Line::from(vec![
            Span::styled("  a         ", Style::default().fg(Color::Cyan)),
            Span::raw("Select account filter"),
        ]),
        Line::from(vec![
            Span::styled("  Esc       ", Style::default().fg(Color::Cyan)),
            Span::raw("Close filter / collapse"),
        ]),
        Line::from(vec![
            Span::styled("  ?         ", Style::default().fg(Color::Cyan)),
            Span::raw("Toggle this help"),
        ]),
        Line::from(vec![
            Span::styled("  q         ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ]),
    ];

    let help = Paragraph::new(help_text).alignment(Alignment::Left).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Help ")
            .border_style(Style::default().fg(Color::Yellow)),
    );

    frame.render_widget(Clear, area);
    frame.render_widget(help, area);
}

/// Create a centered rect of the given percentage width and height.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(Flex::Center)
        .split(area);
    let horizontal = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .split(vertical[0]);
    horizontal[0]
}
