use crate::db::{self, ClassificationCount, EnforcementRow, FullRuleRow};
use sqlx::SqlitePool;

/// Which pane is currently active in the Policy tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyPane {
    Enforcements,
    Rules,
}

/// State for the Policy tab.
pub struct PolicyState {
    pub classification_summary: Vec<ClassificationCount>,
    pub enforcements: Vec<EnforcementRow>,
    pub rules: Vec<FullRuleRow>,
    pub active_pane: PolicyPane,
    pub enforcement_selected: usize,
    pub rule_selected: usize,
    pub loaded: bool,
}

impl PolicyState {
    pub fn new() -> Self {
        Self {
            classification_summary: Vec::new(),
            enforcements: Vec::new(),
            rules: Vec::new(),
            active_pane: PolicyPane::Enforcements,
            enforcement_selected: 0,
            rule_selected: 0,
            loaded: false,
        }
    }

    /// Load policy data from the database.
    pub async fn load(&mut self, pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
        self.classification_summary = db::classification_summary(pool, None).await?;
        self.enforcements = db::recent_enforcements(pool, 100).await?;
        self.rules = db::list_rules(pool, false).await?;
        self.enforcement_selected = 0;
        self.rule_selected = 0;
        self.loaded = true;
        Ok(())
    }

    /// Toggle between Enforcements and Rules pane.
    pub fn next_pane(&mut self) {
        self.active_pane = match self.active_pane {
            PolicyPane::Enforcements => PolicyPane::Rules,
            PolicyPane::Rules => PolicyPane::Enforcements,
        };
    }

    /// Move selection down in the active pane.
    pub fn next(&mut self) {
        match self.active_pane {
            PolicyPane::Enforcements => {
                if !self.enforcements.is_empty() {
                    self.enforcement_selected =
                        (self.enforcement_selected + 1).min(self.enforcements.len() - 1);
                }
            }
            PolicyPane::Rules => {
                if !self.rules.is_empty() {
                    self.rule_selected = (self.rule_selected + 1).min(self.rules.len() - 1);
                }
            }
        }
    }

    /// Move selection up in the active pane.
    pub fn prev(&mut self) {
        match self.active_pane {
            PolicyPane::Enforcements => {
                self.enforcement_selected = self.enforcement_selected.saturating_sub(1);
            }
            PolicyPane::Rules => {
                self.rule_selected = self.rule_selected.saturating_sub(1);
            }
        }
    }

    /// Jump to top of the active pane.
    pub fn top(&mut self) {
        match self.active_pane {
            PolicyPane::Enforcements => self.enforcement_selected = 0,
            PolicyPane::Rules => self.rule_selected = 0,
        }
    }

    /// Jump to bottom of the active pane.
    pub fn bottom(&mut self) {
        match self.active_pane {
            PolicyPane::Enforcements => {
                if !self.enforcements.is_empty() {
                    self.enforcement_selected = self.enforcements.len() - 1;
                }
            }
            PolicyPane::Rules => {
                if !self.rules.is_empty() {
                    self.rule_selected = self.rules.len() - 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_cycling() {
        let mut state = PolicyState::new();
        assert_eq!(state.active_pane, PolicyPane::Enforcements);
        state.next_pane();
        assert_eq!(state.active_pane, PolicyPane::Rules);
        state.next_pane();
        assert_eq!(state.active_pane, PolicyPane::Enforcements);
    }

    #[test]
    fn test_navigation_empty() {
        let mut state = PolicyState::new();
        // Should not panic on empty lists
        state.next();
        state.prev();
        state.top();
        state.bottom();
        assert_eq!(state.enforcement_selected, 0);
        assert_eq!(state.rule_selected, 0);
    }

    #[test]
    fn test_enforcement_navigation() {
        let mut state = PolicyState::new();
        // Simulate 3 enforcements
        for i in 0..3 {
            state.enforcements.push(EnforcementRow {
                id: i,
                timestamp: format!("2026-03-{:02}T00:00:00Z", i + 1),
                session_id: "sess".to_string(),
                tool_name: "Bash".to_string(),
                tool_input: None,
                action: "denied".to_string(),
                reason: None,
                rule_id: None,
            });
        }
        state.active_pane = PolicyPane::Enforcements;

        state.next();
        assert_eq!(state.enforcement_selected, 1);
        state.next();
        assert_eq!(state.enforcement_selected, 2);
        state.next(); // clamped
        assert_eq!(state.enforcement_selected, 2);

        state.prev();
        assert_eq!(state.enforcement_selected, 1);

        state.top();
        assert_eq!(state.enforcement_selected, 0);

        state.bottom();
        assert_eq!(state.enforcement_selected, 2);
    }

    #[test]
    fn test_rule_navigation() {
        let mut state = PolicyState::new();
        for i in 0..2 {
            state.rules.push(FullRuleRow {
                id: i,
                tool_pattern: "^Bash$".to_string(),
                input_pattern: None,
                action: "deny".to_string(),
                reason: "test".to_string(),
                priority: 100,
                enabled: true,
                source: "manual".to_string(),
                created_at: "2026-03-01T00:00:00Z".to_string(),
            });
        }
        state.active_pane = PolicyPane::Rules;

        state.next();
        assert_eq!(state.rule_selected, 1);
        state.next(); // clamped
        assert_eq!(state.rule_selected, 1);
        state.prev();
        assert_eq!(state.rule_selected, 0);
        state.bottom();
        assert_eq!(state.rule_selected, 1);
        state.top();
        assert_eq!(state.rule_selected, 0);
    }

    #[test]
    fn test_pane_navigation_independence() {
        let mut state = PolicyState::new();
        // Add data to both panes
        for i in 0..3 {
            state.enforcements.push(EnforcementRow {
                id: i,
                timestamp: String::new(),
                session_id: String::new(),
                tool_name: String::new(),
                tool_input: None,
                action: "denied".to_string(),
                reason: None,
                rule_id: None,
            });
        }
        for i in 0..2 {
            state.rules.push(FullRuleRow {
                id: i,
                tool_pattern: String::new(),
                input_pattern: None,
                action: "deny".to_string(),
                reason: String::new(),
                priority: 0,
                enabled: true,
                source: String::new(),
                created_at: String::new(),
            });
        }

        // Move in enforcements pane
        state.active_pane = PolicyPane::Enforcements;
        state.next();
        state.next();
        assert_eq!(state.enforcement_selected, 2);

        // Switch to rules, enforcement selection preserved
        state.next_pane();
        state.next();
        assert_eq!(state.rule_selected, 1);
        assert_eq!(state.enforcement_selected, 2); // unchanged
    }
}
