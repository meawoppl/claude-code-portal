//! Command history management for SessionView

/// Maximum number of commands to keep in history
pub const MAX_HISTORY: usize = 100;

/// Command history state
#[derive(Default)]
pub struct CommandHistory {
    /// History entries (most recent last)
    entries: Vec<String>,
    /// Current position in history (None = new input, Some(i) = viewing entries[i])
    position: Option<usize>,
    /// Draft input preserved when navigating history
    draft: String,
}

impl CommandHistory {
    /// Create a new empty command history
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a command to history (avoids consecutive duplicates)
    pub fn push(&mut self, command: String) {
        if self.entries.last() != Some(&command) {
            self.entries.push(command);
            if self.entries.len() > MAX_HISTORY {
                self.entries.remove(0);
            }
        }
        // Reset navigation
        self.position = None;
        self.draft.clear();
    }

    /// Navigate up (older) in history
    /// Returns the command to display, or None if no change
    pub fn navigate_up(&mut self, current_input: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }

        match self.position {
            None => {
                // First time pressing up - save current input as draft
                self.draft = current_input.to_string();
                let pos = self.entries.len() - 1;
                self.position = Some(pos);
                Some(self.entries[pos].clone())
            }
            Some(pos) if pos > 0 => {
                // Go to older command
                let new_pos = pos - 1;
                self.position = Some(new_pos);
                Some(self.entries[new_pos].clone())
            }
            _ => {
                // Already at oldest
                None
            }
        }
    }

    /// Navigate down (newer) in history
    /// Returns the command to display, or None if no change
    pub fn navigate_down(&mut self) -> Option<String> {
        match self.position {
            Some(pos) if pos < self.entries.len() - 1 => {
                // Go to newer command
                let new_pos = pos + 1;
                self.position = Some(new_pos);
                Some(self.entries[new_pos].clone())
            }
            Some(_) => {
                // At newest history entry, go back to draft
                self.position = None;
                Some(self.draft.clone())
            }
            None => {
                // Not in history mode
                None
            }
        }
    }

    /// Reset navigation state
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.position = None;
        self.draft.clear();
    }

    /// Check if currently navigating history
    #[allow(dead_code)]
    pub fn is_navigating(&self) -> bool {
        self.position.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_navigate() {
        let mut history = CommandHistory::new();
        history.push("first".to_string());
        history.push("second".to_string());

        // Navigate up from empty input
        assert_eq!(history.navigate_up(""), Some("second".to_string()));
        assert_eq!(history.navigate_up(""), Some("first".to_string()));
        assert_eq!(history.navigate_up(""), None); // at oldest

        // Navigate down
        assert_eq!(history.navigate_down(), Some("second".to_string()));
        assert_eq!(history.navigate_down(), Some("".to_string())); // back to draft
    }

    #[test]
    fn test_preserves_draft() {
        let mut history = CommandHistory::new();
        history.push("old".to_string());

        // Start typing, then navigate up
        assert_eq!(history.navigate_up("my draft"), Some("old".to_string()));

        // Navigate back down to get draft
        assert_eq!(history.navigate_down(), Some("my draft".to_string()));
    }

    #[test]
    fn test_no_consecutive_duplicates() {
        let mut history = CommandHistory::new();
        history.push("same".to_string());
        history.push("same".to_string());
        history.push("same".to_string());

        assert_eq!(history.navigate_up(""), Some("same".to_string()));
        assert_eq!(history.navigate_up(""), None); // only one entry
    }
}
