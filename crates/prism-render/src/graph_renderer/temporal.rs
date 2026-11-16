//! Temporal anti-aliasing (TAA) jitter and motion-vector history state.

use std::collections::HashMap;

use crate::descriptor::FrameUBOData;

#[derive(Default)]
pub(super) struct TemporalState {
    pub(super) taa_frame: u64,
    pub(super) taa_jitter_ndc: [f32; 2],
    pub(super) prev_camera_view_proj: Option<[[f32; 4]; 4]>,
    pub(super) prev_models: HashMap<u32, [[f32; 4]; 4]>,
}

impl TemporalState {
    pub(super) fn reset_history(&mut self) {
        self.prev_camera_view_proj = None;
        self.prev_models.clear();
    }

    pub(super) fn apply_jitter(&mut self, frame_data: &mut FrameUBOData, active: bool) {
        let jitter = if active {
            let j = taa_jitter(self.taa_frame);
            self.taa_frame = self.taa_frame.wrapping_add(1);
            j
        } else {
            self.taa_frame = 0;
            [0.0, 0.0]
        };
        let width = frame_data.viewportW.max(1.0);
        let height = frame_data.viewportH.max(1.0);
        let jitter_ndc = [jitter[0] * 4.0 / width, jitter[1] * 4.0 / height];
        frame_data.viewProj[3][0] += jitter_ndc[0];
        frame_data.viewProj[3][1] += jitter_ndc[1];
        self.taa_jitter_ndc = jitter_ndc;
    }
}

/// One Halton sequence value in `[0, 1)` (#883).
fn halton(mut index: u64, base: u64) -> f32 {
    let mut result = 0.0;
    let mut factor = 1.0;
    while index > 0 {
        factor /= base as f32;
        result += factor * (index % base) as f32;
        index /= base;
    }
    result
}

/// Subpixel TAA jitter in pixels, centred around zero.
fn taa_jitter(frame: u64) -> [f32; 2] {
    // Halton(2,3), shifted by -0.5 so the sequence covers [-0.5, 0.5).
    [halton(frame + 1, 2) - 0.5, halton(frame + 1, 3) - 0.5]
}
