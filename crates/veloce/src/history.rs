//! Back / forward history for one tab, like a browser's.

/// Entries kept per tab; the oldest are dropped past this.
const MAX_ENTRIES: usize = 100;

#[derive(Debug, Clone)]
pub struct History<T> {
    entries: Vec<T>,
    /// Position of the current entry.
    index: usize,
}

impl<T: Clone + PartialEq> History<T> {
    pub fn new(start: T) -> Self {
        Self {
            entries: vec![start],
            index: 0,
        }
    }

    /// Visit `value`: everything ahead of the current entry is discarded, as in a
    /// browser. Revisiting the current entry (a reload) changes nothing.
    pub fn push(&mut self, value: T) {
        if self.entries[self.index] == value {
            return;
        }
        self.entries.truncate(self.index + 1);
        self.entries.push(value);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.index = self.entries.len() - 1;
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.entries.len()
    }

    /// Step back and return the entry now current.
    pub fn back(&mut self) -> Option<T> {
        if !self.can_go_back() {
            return None;
        }
        self.index -= 1;
        Some(self.entries[self.index].clone())
    }

    /// Step forward and return the entry now current.
    pub fn forward(&mut self) -> Option<T> {
        if !self.can_go_forward() {
            return None;
        }
        self.index += 1;
        Some(self.entries[self.index].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_history_has_nowhere_to_go() {
        let mut history = History::new("a");
        assert!(!history.can_go_back());
        assert!(!history.can_go_forward());
        assert_eq!(history.back(), None);
        assert_eq!(history.forward(), None);
    }

    #[test]
    fn pushing_then_going_back_and_forward_walks_the_trail() {
        let mut history = History::new("a");
        history.push("b");
        history.push("c");
        assert_eq!(history.back(), Some("b"));
        assert_eq!(history.back(), Some("a"));
        assert_eq!(history.back(), None);
        assert_eq!(history.forward(), Some("b"));
        assert_eq!(history.forward(), Some("c"));
        assert_eq!(history.forward(), None);
    }

    #[test]
    fn visiting_somewhere_new_drops_the_forward_trail() {
        let mut history = History::new("a");
        history.push("b");
        history.push("c");
        history.back();
        history.back();
        history.push("z");
        assert!(!history.can_go_forward());
        assert_eq!(history.back(), Some("a"));
    }

    #[test]
    fn revisiting_the_current_entry_is_not_a_new_stop() {
        let mut history = History::new("a");
        history.push("a");
        assert!(!history.can_go_back());
        history.push("b");
        history.push("b");
        assert_eq!(history.back(), Some("a"));
        assert_eq!(history.back(), None);
    }

    #[test]
    fn the_oldest_entries_fall_off_past_the_cap() {
        let mut history = History::new(0u32);
        for n in 1..=(MAX_ENTRIES as u32 + 20) {
            history.push(n);
        }
        let mut steps = 0;
        while history.back().is_some() {
            steps += 1;
        }
        assert_eq!(steps, MAX_ENTRIES - 1);
    }

    #[test]
    fn going_back_does_not_disturb_what_is_ahead() {
        let mut history = History::new("a");
        history.push("b");
        history.back();
        assert!(history.can_go_forward());
        assert!(!history.can_go_back());
    }
}
