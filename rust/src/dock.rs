//! The five-slot quick-access dock. Screen-fixed; this is only the slot model.

use serde::{Deserialize, Serialize};

pub const SLOTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Dock {
    slots: Vec<Option<String>>,
}

impl Dock {
    pub fn new() -> Self {
        Dock {
            slots: vec![None; SLOTS],
        }
    }

    /// Restore from persisted data of any length: extra entries are dropped,
    /// missing ones become empty, and a key may appear only once.
    pub fn normalized(mut self) -> Self {
        self.slots.resize(SLOTS, None);
        let mut seen: Vec<String> = Vec::new();
        for slot in &mut self.slots {
            if let Some(k) = slot.as_ref() {
                if seen.contains(k) {
                    *slot = None;
                } else {
                    seen.push(k.clone());
                }
            }
        }
        self
    }

    pub fn get(&self, i: usize) -> Option<&str> {
        self.slots.get(i).and_then(|s| s.as_deref())
    }

    pub fn slots(&self) -> &[Option<String>] {
        &self.slots
    }

    pub fn position_of(&self, key: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.as_deref() == Some(key))
    }

    pub fn first_empty(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }

    /// Put `key` into `slot`, replacing what was there. If `key` is already
    /// pinned elsewhere it moves (swapping with the target's old content).
    pub fn assign(&mut self, slot: usize, key: &str) -> bool {
        if slot >= SLOTS {
            return false;
        }
        if let Some(from) = self.position_of(key) {
            self.slots.swap(from, slot);
        } else {
            self.slots[slot] = Some(key.to_owned());
        }
        true
    }

    pub fn remove(&mut self, slot: usize) -> bool {
        match self.slots.get_mut(slot) {
            Some(s) if s.is_some() => {
                *s = None;
                true
            }
            _ => false,
        }
    }

    /// Drag-reorder: move the entry at `from` to `to`, shifting the ones between.
    pub fn move_slot(&mut self, from: usize, to: usize) -> bool {
        if from >= SLOTS || to >= SLOTS || from == to {
            return false;
        }
        let item = self.slots.remove(from);
        self.slots.insert(to, item);
        true
    }

    pub fn clear_missing(&mut self, mut exists: impl FnMut(&str) -> bool) {
        for s in &mut self.slots {
            if s.as_deref().is_some_and(|k| !exists(k)) {
                *s = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_five_slots() {
        assert_eq!(Dock::new().slots().len(), 5);
        assert_eq!(Dock::default().normalized().slots().len(), 5);
    }

    #[test]
    fn assign_and_replace() {
        let mut d = Dock::new();
        assert!(d.assign(2, "a"));
        assert!(d.assign(2, "b"));
        assert_eq!(d.get(2), Some("b"));
        assert_eq!(d.position_of("a"), None);
        assert!(!d.assign(5, "x"));
    }

    #[test]
    fn assigning_pinned_app_moves_it() {
        let mut d = Dock::new();
        d.assign(0, "a");
        d.assign(3, "b");
        d.assign(3, "a");
        assert_eq!(d.get(3), Some("a"));
        assert_eq!(d.get(0), Some("b"));
    }

    #[test]
    fn remove_and_first_empty() {
        let mut d = Dock::new();
        d.assign(0, "a");
        d.assign(1, "b");
        assert_eq!(d.first_empty(), Some(2));
        assert!(d.remove(0));
        assert!(!d.remove(0));
        assert_eq!(d.first_empty(), Some(0));
    }

    #[test]
    fn reorder() {
        let mut d = Dock::new();
        for (i, k) in ["a", "b", "c"].iter().enumerate() {
            d.assign(i, k);
        }
        assert!(d.move_slot(0, 2));
        assert_eq!(d.get(0), Some("b"));
        assert_eq!(d.get(1), Some("c"));
        assert_eq!(d.get(2), Some("a"));
        assert!(!d.move_slot(1, 1));
        assert!(!d.move_slot(0, 9));
    }

    #[test]
    fn normalize_fixes_bad_data() {
        let d = Dock {
            slots: vec![Some("a".into()), Some("a".into()), None, None, None, Some("z".into())],
        }
        .normalized();
        assert_eq!(d.slots().len(), 5);
        assert_eq!(d.get(1), None);
        let short = Dock {
            slots: vec![Some("a".into())],
        }
        .normalized();
        assert_eq!(short.slots().len(), 5);
    }

    #[test]
    fn clear_missing_drops_uninstalled() {
        let mut d = Dock::new();
        d.assign(0, "a");
        d.assign(1, "b");
        d.clear_missing(|k| k == "a");
        assert_eq!(d.get(0), Some("a"));
        assert_eq!(d.get(1), None);
    }

    #[test]
    fn serde_round_trip() {
        let mut d = Dock::new();
        d.assign(1, "x");
        let s = serde_json::to_string(&d).unwrap();
        let back: Dock = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
