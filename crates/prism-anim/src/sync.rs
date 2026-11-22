//! Sync group / Sync marker (#560 P1).
//!
//! "Phase" for looping clips like walking/running cannot be aligned solely by normalized time:
//! footfall position may land at 0.18 / 0.62 in different clips, not the same 0.0. Sync group
//! uses a set of markers to map **leader clip phase** segment-by-segment to **follower clip phase**,
//! so both feet step on the same beat when mixing.
//!
//! This layer only does math: markers are normalized phase `[0, 1)`, independent of clip / asset / ECS.
//! Graph evaluator and blend space can call [`SyncGroup::map_phase`] before sampling.
//!
//! Convention:
//!
//! - Each clip must have the same number of markers; markers sorted by ascending phase, duplicates removed;
//! - Mapping linearly interpolates between adjacent markers; last segment wraps from last marker back to first;
//! - When there is only 1 marker, all phases map to that marker;
//! - Non-finite phase / out-of-bounds clip index / illegal group safely fall back (return leader phase or `None`).

/// A sync marker set for one clip, phase normalized to `[0, 1)`.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncMarkerSet {
    phases: Vec<f32>,
}

impl SyncMarkerSet {
    /// Construct from marker phases. Non-finite values discarded, phase normalized via `rem_euclid(1)`,
    /// sorted and deduplicated; empty collection returns `None`.
    pub fn new(phases: impl IntoIterator<Item = f32>) -> Option<Self> {
        let mut phases: Vec<f32> = phases
            .into_iter()
            .filter(|phase| phase.is_finite())
            .map(|phase| phase.rem_euclid(1.0))
            .collect();
        phases.sort_by(f32::total_cmp);
        phases.dedup_by(|a, b| (*a - *b).abs() <= 1e-6);
        if phases.is_empty() {
            None
        } else {
            Some(Self { phases })
        }
    }

    /// Ascending, deduplicated marker phases.
    pub fn phases(&self) -> &[f32] {
        &self.phases
    }

    pub fn len(&self) -> usize {
        self.phases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.phases.is_empty()
    }
}

/// A set of clips sharing a marker structure. Index 0 usually serves as the leader clip,
/// but [`Self::map_phase`] allows arbitrary index conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncGroup {
    sets: Vec<SyncMarkerSet>,
}

impl SyncGroup {
    /// Construct sync group; all clips must have the same number of markers and non-empty.
    pub fn new(sets: Vec<SyncMarkerSet>) -> Option<Self> {
        let count = sets.first()?.len();
        if sets.iter().any(|set| set.len() != count) {
            None
        } else {
            Some(Self { sets })
        }
    }

    pub fn len(&self) -> usize {
        self.sets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    pub fn markers(&self, clip: usize) -> Option<&SyncMarkerSet> {
        self.sets.get(clip)
    }

    /// Maps normalized phase of `from` clip to `to` clip.
    ///
    /// Out-of-bounds index / non-finite phase returns `phase` as-is (caller can keep sampling,
    /// one bad marker won't break the whole mix).
    pub fn map_phase(&self, from: usize, to: usize, phase: f32) -> f32 {
        if !phase.is_finite() || from >= self.sets.len() || to >= self.sets.len() {
            return phase;
        }
        let phase = phase.rem_euclid(1.0);
        let leader = &self.sets[from].phases;
        let follower = &self.sets[to].phases;
        let count = leader.len();
        if count <= 1 {
            return follower[0];
        }
        if from == to {
            return phase;
        }

        // Find the segment of the leader clip containing `phase`; `phase` before the first marker belongs
        // to the last segment (phase + 1 compared with the wrap-around segment).
        let (segment, local) = if phase < leader[0] {
            let start = leader[count - 1];
            let end = leader[0] + 1.0;
            (count - 1, (phase + 1.0 - start) / (end - start))
        } else {
            let mut segment = count - 1;
            for index in 0..count - 1 {
                if phase >= leader[index] && phase < leader[index + 1] {
                    segment = index;
                    break;
                }
            }
            let start = leader[segment];
            let end = if segment + 1 < count {
                leader[segment + 1]
            } else {
                leader[0] + 1.0
            };
            (segment, (phase - start) / (end - start))
        };

        let start = follower[segment];
        let end = if segment + 1 < count {
            follower[segment + 1]
        } else {
            follower[0] + 1.0
        };
        (start + local.clamp(0.0, 1.0) * (end - start)).rem_euclid(1.0)
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
