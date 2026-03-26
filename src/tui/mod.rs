mod app;
mod event;
mod help;
mod tabs;
mod ui;

use std::io::{self, stdout};
use std::time::Duration;

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use sqlx::SqlitePool;

use app::{App, Tab};
use crossterm::event::KeyCode;
use event::{AppEvent, EventHandler};

/// Run the TUI application.
pub async fn run(
    pool: &SqlitePool,
    db_path: &str,
    tick_rate: Duration,
    since: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Install panic hook that restores terminal state
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    // Create app state and event handler
    let mut app = App::new(tick_rate, since, db_path.to_string());
    let mut events = EventHandler::new(tick_rate);

    // Main event loop
    loop {
        // Lazy-load data for the active tab
        if app.active_tab == Tab::Sessions && !app.sessions.loaded {
            let _ = app.sessions.load(pool, app.since.as_deref()).await;
        }
        if app.active_tab == Tab::Events && !app.events.loaded {
            let _ = app.events.load(pool, app.since.as_deref()).await;
        }
        if app.active_tab == Tab::Stats && !app.stats.loaded {
            let _ = app
                .stats
                .load(pool, &app.db_path, app.since.as_deref())
                .await;
        }

        terminal.draw(|frame| ui::draw(frame, &app))?;

        match events.next().await {
            Some(AppEvent::Key(key)) => {
                // Help overlay captures Esc and ? only
                if app.show_help {
                    match key.code {
                        KeyCode::Char('?') | KeyCode::Esc => app.toggle_help(),
                        _ => {}
                    }
                    continue;
                }

                // Global keybindings
                match key.code {
                    KeyCode::Char('q') => app.quit(),
                    KeyCode::Char('?') => app.toggle_help(),
                    KeyCode::Char('1') => app.set_tab(Tab::Sessions),
                    KeyCode::Char('2') => app.set_tab(Tab::Events),
                    KeyCode::Char('3') => {
                        app.stats.loaded = false; // refresh on switch
                        app.set_tab(Tab::Stats);
                    }
                    KeyCode::Char('4') => app.set_tab(Tab::Live),
                    KeyCode::Tab
                        if app.active_tab == Tab::Events && app.events.expanded.is_some() =>
                    {
                        // Tab toggles detail mode when detail is expanded
                        app.events.toggle_detail_mode();
                    }
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::BackTab => app.prev_tab(),
                    // Tab-specific keybindings
                    _ => match app.active_tab {
                        Tab::Sessions => handle_sessions_key(&mut app, key.code),
                        Tab::Events => handle_events_key(&mut app, key.code),
                        Tab::Stats => handle_stats_key(&mut app, key.code),
                        _ => {}
                    },
                }
            }
            Some(AppEvent::Tick) => {
                // Live tab polling will be added in US-0031
            }
            Some(AppEvent::Resize(_, _)) => {
                // ratatui handles resize automatically on next draw
            }
            None => break, // channel closed
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

/// Handle key events specific to the Sessions tab.
fn handle_sessions_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Down | KeyCode::Char('j') => app.sessions.next(),
        KeyCode::Up | KeyCode::Char('k') => app.sessions.prev(),
        KeyCode::Char('g') => app.sessions.top(),
        KeyCode::Char('G') => app.sessions.bottom(),
        KeyCode::Enter => {
            // Drill down to Events tab filtered to this session
            if let Some(session_id) = app.sessions.selected_session_id() {
                app.events.set_session_filter(session_id.to_string());
                app.set_tab(Tab::Events);
            }
        }
        _ => {}
    }
}

/// Handle key events specific to the Events tab.
fn handle_events_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Down | KeyCode::Char('j') => app.events.next(),
        KeyCode::Up | KeyCode::Char('k') => app.events.prev(),
        KeyCode::Char('g') => app.events.top(),
        KeyCode::Char('G') => app.events.bottom(),
        KeyCode::Enter => app.events.toggle_expand(),
        KeyCode::Esc => {
            if app.events.expanded.is_some() {
                app.events.expanded = None;
            } else if app.events.session_filter.is_some() {
                app.events.clear_session_filter();
            }
        }
        KeyCode::Backspace => {
            if app.events.session_filter.is_some() {
                app.events.clear_session_filter();
            }
        }
        _ => {}
    }
}

/// Handle key events specific to the Stats tab.
fn handle_stats_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Down | KeyCode::Char('j') => app.stats.scroll_down(),
        KeyCode::Up | KeyCode::Char('k') => app.stats.scroll_up(),
        KeyCode::Char('g') => app.stats.scroll_top(),
        KeyCode::Char('G') => app.stats.scroll_bottom(),
        _ => {}
    }
}
