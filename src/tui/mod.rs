mod app;
mod event;
pub(crate) mod filter;
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
        if app.active_tab == Tab::Live && !app.live.initialized {
            let _ = app.live.initialize(pool).await;
        }
        if app.active_tab == Tab::Policy && !app.policy.loaded {
            let _ = app.policy.load(pool).await;
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

                // Filter bar captures all input when active
                if app.filter.active {
                    match key.code {
                        KeyCode::Esc => app.filter.deactivate(),
                        KeyCode::Enter => {
                            // Confirm filter — keep text, return focus to list
                            app.filter.active = false;
                        }
                        KeyCode::Backspace => app.filter.delete_char(),
                        KeyCode::Char(c) => app.filter.push_char(c),
                        _ => {}
                    }
                    continue;
                }

                // Global keybindings
                match key.code {
                    KeyCode::Char('q') => app.quit(),
                    KeyCode::Char('?') => app.toggle_help(),
                    KeyCode::Char('/') if matches!(app.active_tab, Tab::Sessions | Tab::Events) => {
                        app.filter.activate();
                    }
                    KeyCode::Char('1') => app.set_tab(Tab::Sessions),
                    KeyCode::Char('2') => app.set_tab(Tab::Events),
                    KeyCode::Char('3') => {
                        app.stats.loaded = false; // refresh on switch
                        app.set_tab(Tab::Stats);
                    }
                    KeyCode::Char('4') => app.set_tab(Tab::Live),
                    KeyCode::Char('5') => {
                        app.policy.loaded = false; // refresh on switch
                        app.set_tab(Tab::Policy);
                    }
                    KeyCode::Tab
                        if app.active_tab == Tab::Events && app.events.expanded.is_some() =>
                    {
                        // Tab toggles detail mode when detail is expanded
                        app.events.toggle_detail_mode();
                    }
                    KeyCode::Tab if app.active_tab == Tab::Policy => {
                        // Tab cycles pane within the Policy tab
                        app.policy.next_pane();
                    }
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::BackTab => app.prev_tab(),
                    // Tab-specific keybindings
                    _ => match app.active_tab {
                        Tab::Sessions => handle_sessions_key(&mut app, key.code),
                        Tab::Events => handle_events_key(&mut app, key.code),
                        Tab::Stats => handle_stats_key(&mut app, key.code),
                        Tab::Live => handle_live_key(&mut app, key.code),
                        Tab::Policy => handle_policy_key(&mut app, key.code),
                    },
                }
            }
            Some(AppEvent::Tick) => {
                if app.active_tab == Tab::Live && app.live.initialized {
                    let _ = app.live.poll(pool).await;
                }
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

/// Handle key events specific to the Live tab.
fn handle_live_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Up | KeyCode::Char('k') => app.live.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.live.scroll_down(),
        KeyCode::Char('G') => app.live.scroll_to_bottom(),
        _ => {}
    }
}

/// Handle key events specific to the Policy tab.
fn handle_policy_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Down | KeyCode::Char('j') => app.policy.next(),
        KeyCode::Up | KeyCode::Char('k') => app.policy.prev(),
        KeyCode::Char('g') => app.policy.top(),
        KeyCode::Char('G') => app.policy.bottom(),
        _ => {}
    }
}
