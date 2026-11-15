//! Motion Matching Chooser and LOD policy (GASP P3).

/// Character gait tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaitMode {
    Idle,
    Walk,
    Run,
    Sprint,
    Crouch,
}

/// Character stance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StanceMode {
    Standing,
    Crouching,
    InAir,
}

/// Motion Matching search LOD tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionMatchingLod {
    Lod0Full,
    Lod1Reduced,
    Lod2Minimal,
}

impl MotionMatchingLod {
    pub fn search_stride(self) -> usize {
        match self {
            MotionMatchingLod::Lod0Full => 1,
            MotionMatchingLod::Lod1Reduced => 2,
            MotionMatchingLod::Lod2Minimal => 4,
        }
    }

    pub fn max_candidates(self) -> usize {
        match self {
            MotionMatchingLod::Lod0Full => usize::MAX,
            MotionMatchingLod::Lod1Reduced => 512,
            MotionMatchingLod::Lod2Minimal => 128,
        }
    }
}

/// Runtime context evaluated by the Chooser.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChooserContext {
    pub gait: GaitMode,
    pub stance: StanceMode,
    pub speed: f32,
    pub is_pivoting: bool,
    pub lod: MotionMatchingLod,
}

/// A candidate Motion Matching database entry in the Chooser table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChooserEntry {
    pub database_index: usize,
    pub gait: GaitMode,
    pub stance: StanceMode,
    pub min_speed: f32,
    pub max_speed: f32,
}

/// Select the best matching database index for `ctx`.
pub fn evaluate_chooser(entries: &[ChooserEntry], ctx: &ChooserContext) -> Option<usize> {
    entries
        .iter()
        .find(|e| {
            e.gait == ctx.gait
                && e.stance == ctx.stance
                && ctx.speed >= e.min_speed
                && ctx.speed <= e.max_speed
        })
        .map(|e| e.database_index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooser_selects_database_by_gait_stance_and_speed() {
        let entries = [
            ChooserEntry {
                database_index: 0,
                gait: GaitMode::Idle,
                stance: StanceMode::Standing,
                min_speed: 0.0,
                max_speed: 0.2,
            },
            ChooserEntry {
                database_index: 1,
                gait: GaitMode::Walk,
                stance: StanceMode::Standing,
                min_speed: 0.2,
                max_speed: 2.5,
            },
            ChooserEntry {
                database_index: 2,
                gait: GaitMode::Run,
                stance: StanceMode::Standing,
                min_speed: 2.5,
                max_speed: 6.0,
            },
        ];
        let ctx = ChooserContext {
            gait: GaitMode::Run,
            stance: StanceMode::Standing,
            speed: 4.0,
            is_pivoting: false,
            lod: MotionMatchingLod::Lod0Full,
        };
        assert_eq!(evaluate_chooser(&entries, &ctx), Some(2));
        assert_eq!(MotionMatchingLod::Lod1Reduced.search_stride(), 2);
        assert_eq!(MotionMatchingLod::Lod2Minimal.max_candidates(), 128);
    }
}
