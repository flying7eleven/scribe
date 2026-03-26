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
    _pool: &SqlitePool,
    _db_path: &str,
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
    let mut app = App::new(tick_rate, since);
    let mut events = EventHandler::new(tick_rate);

    // Main event loop
    loop {
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
                    KeyCode::Char('3') => app.set_tab(Tab::Stats),
                    KeyCode::Char('4') => app.set_tab(Tab::Live),
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::BackTab => app.prev_tab(),
                    // Tab-specific keybindings will be added in later stories
                    _ => {}
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
