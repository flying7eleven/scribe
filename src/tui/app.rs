use std::time::Duration;

use super::filter::FilterState;
use super::tabs::events::EventsState;
use super::tabs::live::LiveState;
use super::tabs::policy::PolicyState;
use super::tabs::sessions::SessionsState;
use super::tabs::stats::StatsState;

/// The five navigable tabs in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Sessions,
    Events,
    Stats,
    Live,
    Policy,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Sessions,
        Tab::Events,
        Tab::Stats,
        Tab::Live,
        Tab::Policy,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Sessions => "Sessions",
            Tab::Events => "Events",
            Tab::Stats => "Stats",
            Tab::Live => "Live",
            Tab::Policy => "Policy",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::Sessions => 0,
            Tab::Events => 1,
            Tab::Stats => 2,
            Tab::Live => 3,
            Tab::Policy => 4,
        }
    }

    pub fn from_index(i: usize) -> Option<Tab> {
        match i {
            0 => Some(Tab::Sessions),
            1 => Some(Tab::Events),
            2 => Some(Tab::Stats),
            3 => Some(Tab::Live),
            4 => Some(Tab::Policy),
            _ => None,
        }
    }
}

/// Top-level application state.
pub struct App {
    pub active_tab: Tab,
    pub show_help: bool,
    pub should_quit: bool,
    #[allow(dead_code)] // consumed by Live tab (US-0031)
    pub tick_rate: Duration,
    pub since: Option<String>,
    pub db_path: String,
    pub sessions: SessionsState,
    pub events: EventsState,
    pub stats: StatsState,
    pub live: LiveState,
    pub policy: PolicyState,
    pub filter: FilterState,
}

impl App {
    pub fn new(tick_rate: Duration, since: Option<String>, db_path: String) -> Self {
        Self {
            active_tab: Tab::Sessions,
            show_help: false,
            should_quit: false,
            tick_rate,
            since,
            db_path,
            sessions: SessionsState::new(),
            events: EventsState::new(),
            stats: StatsState::new(),
            live: LiveState::new(),
            policy: PolicyState::new(),
            filter: FilterState::new(),
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.active_tab = tab;
        self.filter.deactivate();
    }

    pub fn next_tab(&mut self) {
        let next = (self.active_tab.index() + 1) % Tab::ALL.len();
        self.active_tab = Tab::from_index(next).unwrap_or(Tab::Sessions);
        self.filter.deactivate();
    }

    pub fn prev_tab(&mut self) {
        let prev = (self.active_tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len();
        self.active_tab = Tab::from_index(prev).unwrap_or(Tab::Sessions);
        self.filter.deactivate();
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_titles() {
        assert_eq!(Tab::Sessions.title(), "Sessions");
        assert_eq!(Tab::Events.title(), "Events");
        assert_eq!(Tab::Stats.title(), "Stats");
        assert_eq!(Tab::Live.title(), "Live");
    }

    #[test]
    fn test_tab_index_roundtrip() {
        for tab in Tab::ALL {
            assert_eq!(Tab::from_index(tab.index()), Some(tab));
        }
        assert_eq!(Tab::from_index(99), None);
    }

    #[test]
    fn test_app_tab_switching() {
        let mut app = App::new(Duration::from_secs(1), None, String::new());
        assert_eq!(app.active_tab, Tab::Sessions);

        app.next_tab();
        assert_eq!(app.active_tab, Tab::Events);

        app.next_tab();
        assert_eq!(app.active_tab, Tab::Stats);

        app.next_tab();
        assert_eq!(app.active_tab, Tab::Live);

        app.next_tab();
        assert_eq!(app.active_tab, Tab::Policy);

        app.next_tab(); // wraps
        assert_eq!(app.active_tab, Tab::Sessions);
    }

    #[test]
    fn test_app_prev_tab_wraps() {
        let mut app = App::new(Duration::from_secs(1), None, String::new());
        assert_eq!(app.active_tab, Tab::Sessions);

        app.prev_tab(); // wraps to Policy
        assert_eq!(app.active_tab, Tab::Policy);

        app.prev_tab();
        assert_eq!(app.active_tab, Tab::Live);
    }

    #[test]
    fn test_app_set_tab() {
        let mut app = App::new(Duration::from_secs(1), None, String::new());
        app.set_tab(Tab::Stats);
        assert_eq!(app.active_tab, Tab::Stats);
    }

    #[test]
    fn test_app_toggle_help() {
        let mut app = App::new(Duration::from_secs(1), None, String::new());
        assert!(!app.show_help);
        app.toggle_help();
        assert!(app.show_help);
        app.toggle_help();
        assert!(!app.show_help);
    }

    #[test]
    fn test_app_quit() {
        let mut app = App::new(Duration::from_secs(1), None, String::new());
        assert!(!app.should_quit);
        app.quit();
        assert!(app.should_quit);
    }
}
