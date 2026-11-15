//! Checkpoints, exploration map, collectibles, and linear narrative flow for EchoWell.

use std::collections::{HashMap, HashSet};

/// Active respawn checkpoint inside a room.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    pub id: String,
    pub room_id: String,
    pub position: [f32; 2],
}

/// Exploration map and collectible tracking.
#[derive(Clone, Debug, Default)]
pub struct ExplorationMap {
    pub visited_rooms: HashSet<String>,
    pub revealed_regions: HashSet<String>,
    pub collectibles: HashSet<String>,
}

impl ExplorationMap {
    pub fn visit_room(&mut self, room_id: &str) {
        self.visited_rooms.insert(room_id.to_string());
    }

    pub fn reveal_region(&mut self, region_id: &str) {
        self.revealed_regions.insert(region_id.to_string());
    }

    pub fn collect(&mut self, item_id: &str) -> bool {
        self.collectibles.insert(item_id.to_string())
    }
}

/// Linear, skippable narrative dialogue/event sequence.
#[derive(Clone, Debug)]
pub struct NarrativeSequence {
    pub id: String,
    pub lines: Vec<String>,
    pub cursor: usize,
    pub completed: bool,
    pub completion_flag: Option<String>,
}

impl NarrativeSequence {
    pub fn new(id: impl Into<String>, lines: Vec<String>, completion_flag: Option<String>) -> Self {
        Self {
            id: id.into(),
            lines,
            cursor: 0,
            completed: false,
            completion_flag,
        }
    }

    pub fn current_line(&self) -> Option<&str> {
        if self.completed {
            None
        } else {
            self.lines.get(self.cursor).map(String::as_str)
        }
    }

    pub fn advance(&mut self, flags: &mut HashMap<String, bool>) {
        if self.completed {
            return;
        }
        self.cursor += 1;
        if self.cursor >= self.lines.len() {
            self.finish(flags);
        }
    }

    pub fn skip(&mut self, flags: &mut HashMap<String, bool>) {
        self.finish(flags);
    }

    fn finish(&mut self, flags: &mut HashMap<String, bool>) {
        self.completed = true;
        self.cursor = self.lines.len();
        if let Some(flag) = &self.completion_flag {
            flags.insert(flag.clone(), true);
        }
    }
}

/// Versioned EchoWell save state.
#[derive(Clone, Debug)]
pub struct SaveState {
    pub version: u32,
    pub active_checkpoint: Option<Checkpoint>,
    pub map: ExplorationMap,
    pub flags: HashMap<String, bool>,
    pub deaths: u32,
    pub playtime_secs: f32,
}

impl Default for SaveState {
    fn default() -> Self {
        Self {
            version: 1,
            active_checkpoint: None,
            map: ExplorationMap::default(),
            flags: HashMap::new(),
            deaths: 0,
            playtime_secs: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_and_exploration_map_track_state() {
        let mut save = SaveState::default();
        save.map.visit_room("well");
        save.map.reveal_region("well");
        assert!(save.map.collect("relic_01"));
        assert!(!save.map.collect("relic_01"));
        save.active_checkpoint = Some(Checkpoint {
            id: "cp_well".into(),
            room_id: "well".into(),
            position: [82.0, 56.0],
        });
        assert_eq!(save.active_checkpoint.as_ref().unwrap().room_id, "well");
    }

    #[test]
    fn narrative_sequence_advances_and_skips_with_flag() {
        let mut flags = HashMap::new();
        let mut seq = NarrativeSequence::new(
            "intro",
            vec!["Line 1".into(), "Line 2".into()],
            Some("intro_done".into()),
        );
        assert_eq!(seq.current_line(), Some("Line 1"));
        seq.advance(&mut flags);
        assert_eq!(seq.current_line(), Some("Line 2"));
        seq.skip(&mut flags);
        assert!(seq.completed);
        assert_eq!(flags.get("intro_done"), Some(&true));
    }
}
