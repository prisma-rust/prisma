//! RTXGI v1.3 DDGI host state, `Texture2DArray` atlas layout, infinite scrolling,
//! and probe variability tracking.
//!
//! Pure CPU-side math and state management for issue #1164. GPU compute passes
//! (`ddgi_clear`, `ddgi_update_irradiance`, `ddgi_update_distance`, `ddgi_blend`,
//! `ddgi_resolve`) consume these descriptors once Slang shaders are compiled.

use crate::capabilities::RayTracingCaps;
use crate::rtxgi_suite::RtxgiBackend;

/// RTXGI v1.3 irradiance interior resolution per probe (excluding 1 px octahedral border).
pub const DDGI_IRRADIANCE_TEXELS: u32 = 6;
/// RTXGI v1.3 irradiance tile size per probe (6x6 interior + 1 px border on each side).
pub const DDGI_IRRADIANCE_TILE_TEXELS: u32 = DDGI_IRRADIANCE_TEXELS + 2;

/// RTXGI v1.3 distance interior resolution per probe (excluding 1 px octahedral border).
pub const DDGI_DISTANCE_TEXELS: u32 = 14;
/// RTXGI v1.3 distance tile size per probe (14x14 interior + 1 px border on each side).
pub const DDGI_DISTANCE_TILE_TEXELS: u32 = DDGI_DISTANCE_TEXELS + 2;

/// RTXGI v1.3 requires rays per probe to be a multiple of 64.
pub const DDGI_RAY_BATCH_MULTIPLE: u32 = 64;

/// Authoring and runtime parameters for a single DDGI probe volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DdgiVolumeParams {
    pub origin: [f32; 3],
    pub rotation_xyzw: [f32; 4],
    pub probe_counts: [u32; 3],
    pub probe_spacing: [f32; 3],
    pub rays_per_probe: u32,
    pub probe_max_ray_distance: f32,
    pub hysteresis: f32,
    pub normal_bias: f32,
    pub view_bias: f32,
    pub irradiance_threshold: f32,
    pub brightness_threshold: f32,
    pub relocation_enabled: bool,
    pub classification_enabled: bool,
    pub scrolling_enabled: bool,
    pub variability_enabled: bool,
}

impl Default for DdgiVolumeParams {
    fn default() -> Self {
        Self {
            origin: [0.0, 2.0, 0.0],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
            probe_counts: [16, 8, 16],
            probe_spacing: [2.0, 2.0, 2.0],
            rays_per_probe: 128,
            probe_max_ray_distance: 64.0,
            hysteresis: 0.97,
            normal_bias: 0.1,
            view_bias: 0.1,
            irradiance_threshold: 0.25,
            brightness_threshold: 2.0,
            relocation_enabled: true,
            classification_enabled: true,
            scrolling_enabled: false,
            variability_enabled: true,
        }
    }
}

impl DdgiVolumeParams {
    /// Total probe count in the 3D grid.
    #[must_use]
    pub const fn total_probes(&self) -> u32 {
        self.probe_counts[0] * self.probe_counts[1] * self.probe_counts[2]
    }

    /// Round `rays_per_probe` up to the nearest RTXGI v1.3 multiple of 64 (min 64).
    #[must_use]
    pub const fn aligned_rays_per_probe(&self) -> u32 {
        let r = if self.rays_per_probe == 0 {
            DDGI_RAY_BATCH_MULTIPLE
        } else {
            self.rays_per_probe
        };
        r.div_ceil(DDGI_RAY_BATCH_MULTIPLE) * DDGI_RAY_BATCH_MULTIPLE
    }
}

/// RTXGI v1.3 `Texture2DArray` atlas dimensions and VRAM estimate for a DDGI volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DdgiAtlasLayout {
    pub irradiance_extent: [u32; 2],
    pub distance_extent: [u32; 2],
    pub array_layers: u32,
    /// Estimated GPU memory in bytes (RGBA16F irradiance + RG16F distance + metadata).
    pub estimated_bytes: u64,
}

impl DdgiAtlasLayout {
    /// Compute the `Texture2DArray` dimensions for `probe_counts = [nx, ny, nz]`.
    #[must_use]
    pub const fn from_probe_counts(probe_counts: [u32; 3]) -> Self {
        let nx = probe_counts[0];
        let ny = probe_counts[1];
        let nz = probe_counts[2];
        let irradiance_extent = [
            nx * DDGI_IRRADIANCE_TILE_TEXELS,
            nz * DDGI_IRRADIANCE_TILE_TEXELS,
        ];
        let distance_extent = [
            nx * DDGI_DISTANCE_TILE_TEXELS,
            nz * DDGI_DISTANCE_TILE_TEXELS,
        ];
        let irr_bytes =
            (irradiance_extent[0] as u64) * (irradiance_extent[1] as u64) * (ny as u64) * 8;
        let dist_bytes =
            (distance_extent[0] as u64) * (distance_extent[1] as u64) * (ny as u64) * 4;
        let metadata_bytes = (nx as u64) * (ny as u64) * (nz as u64) * 8;
        Self {
            irradiance_extent,
            distance_extent,
            array_layers: ny,
            estimated_bytes: irr_bytes + dist_bytes + metadata_bytes,
        }
    }
}

/// Infinite scrolling state for camera-following DDGI volumes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DdgiScrollingState {
    pub snapped_origin: [f32; 3],
    pub grid_coords: [i32; 3],
    pub scroll_offsets: [i32; 3],
}

/// Per-frame delta returned by [`DdgiScrollingState::update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DdgiScrollDelta {
    /// Signed probe-grid step moved since the previous frame on each axis.
    pub step: [i32; 3],
    /// Whether the entire volume should be cleared (jump larger than grid extent).
    pub clear_all: bool,
}

impl DdgiScrollingState {
    /// Initialize scrolling state snapped to the probe grid around `target_world_pos`.
    #[must_use]
    pub fn new(target_world_pos: [f32; 3], spacing: [f32; 3]) -> Self {
        let grid_coords = [
            (target_world_pos[0] / spacing[0].max(1e-4)).round() as i32,
            (target_world_pos[1] / spacing[1].max(1e-4)).round() as i32,
            (target_world_pos[2] / spacing[2].max(1e-4)).round() as i32,
        ];
        Self {
            snapped_origin: [
                grid_coords[0] as f32 * spacing[0],
                grid_coords[1] as f32 * spacing[1],
                grid_coords[2] as f32 * spacing[2],
            ],
            grid_coords,
            scroll_offsets: [0, 0, 0],
        }
    }

    /// Update snapped origin and toroidal scroll offsets for a new target position.
    pub fn update(
        &mut self,
        target_world_pos: [f32; 3],
        spacing: [f32; 3],
        probe_counts: [u32; 3],
    ) -> DdgiScrollDelta {
        let next = [
            (target_world_pos[0] / spacing[0].max(1e-4)).round() as i32,
            (target_world_pos[1] / spacing[1].max(1e-4)).round() as i32,
            (target_world_pos[2] / spacing[2].max(1e-4)).round() as i32,
        ];
        let step = [
            next[0] - self.grid_coords[0],
            next[1] - self.grid_coords[1],
            next[2] - self.grid_coords[2],
        ];
        let clear_all = step[0].unsigned_abs() >= probe_counts[0]
            || step[1].unsigned_abs() >= probe_counts[1]
            || step[2].unsigned_abs() >= probe_counts[2];

        self.grid_coords = next;
        self.snapped_origin = [
            next[0] as f32 * spacing[0],
            next[1] as f32 * spacing[1],
            next[2] as f32 * spacing[2],
        ];
        if clear_all {
            self.scroll_offsets = [0, 0, 0];
        } else {
            for axis in 0..3 {
                let count = probe_counts[axis].max(1) as i32;
                self.scroll_offsets[axis] =
                    (self.scroll_offsets[axis] + step[axis]).rem_euclid(count);
            }
        }
        DdgiScrollDelta { step, clear_all }
    }

    /// Map a logical probe index `[x, y, z]` to its toroidal texel/layer index.
    #[must_use]
    pub const fn logical_to_storage_index(
        &self,
        logical: [u32; 3],
        probe_counts: [u32; 3],
    ) -> [u32; 3] {
        [
            ((logical[0] as i32 + self.scroll_offsets[0]).rem_euclid(probe_counts[0] as i32))
                as u32,
            ((logical[1] as i32 + self.scroll_offsets[1]).rem_euclid(probe_counts[1] as i32))
                as u32,
            ((logical[2] as i32 + self.scroll_offsets[2]).rem_euclid(probe_counts[2] as i32))
                as u32,
        ]
    }
}

/// RTXGI v1.3.5 probe variability convergence state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DdgiVariabilityState {
    pub threshold: f32,
    pub converged_frames: u32,
    pub settle_frames: u32,
    pub asleep: bool,
}

impl Default for DdgiVariabilityState {
    fn default() -> Self {
        Self {
            threshold: 0.005,
            converged_frames: 0,
            settle_frames: 16,
            asleep: false,
        }
    }
}

impl DdgiVariabilityState {
    /// Record this frame's maximum irradiance variability and update sleep state.
    pub fn record_frame(&mut self, max_variability: f32) {
        if max_variability <= self.threshold {
            self.converged_frames = self.converged_frames.saturating_add(1);
            if self.converged_frames >= self.settle_frames {
                self.asleep = true;
            }
        } else {
            self.converged_frames = 0;
            self.asleep = false;
        }
    }

    /// Wake the volume immediately when lights or geometry move.
    pub fn wake(&mut self) {
        self.converged_frames = 0;
        self.asleep = false;
    }

    /// Whether rays should be traced for this volume on the current frame.
    #[must_use]
    pub const fn should_trace(&self) -> bool {
        !self.asleep
    }
}

/// Select the highest-capability RTXGI backend supported by the current device.
#[must_use]
pub fn select_rtxgi_backend(
    requested: RtxgiBackend,
    rt_caps: &RayTracingCaps,
    is_nvidia_tensor_core: bool,
) -> RtxgiBackend {
    if requested == RtxgiBackend::Off || !rt_caps.has_ray_query() {
        return RtxgiBackend::Off;
    }
    match requested {
        RtxgiBackend::Off => RtxgiBackend::Off,
        RtxgiBackend::Ddgi => RtxgiBackend::Ddgi,
        RtxgiBackend::Sharc => RtxgiBackend::Sharc,
        RtxgiBackend::Nrc => {
            if is_nvidia_tensor_core {
                RtxgiBackend::Nrc
            } else {
                RtxgiBackend::Sharc
            }
        }
        RtxgiBackend::FullSuite => {
            if is_nvidia_tensor_core {
                RtxgiBackend::FullSuite
            } else {
                RtxgiBackend::Sharc
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_layout_uses_rtxgi_1_3_tile_borders() {
        let layout = DdgiAtlasLayout::from_probe_counts([16, 8, 16]);
        assert_eq!(layout.irradiance_extent, [16 * 8, 16 * 8]);
        assert_eq!(layout.distance_extent, [16 * 16, 16 * 16]);
        assert_eq!(layout.array_layers, 8);
        assert!(layout.estimated_bytes > 0);
    }

    #[test]
    fn rays_per_probe_align_to_64() {
        let mut params = DdgiVolumeParams {
            rays_per_probe: 100,
            ..Default::default()
        };
        assert_eq!(params.aligned_rays_per_probe(), 128);
        params.rays_per_probe = 0;
        assert_eq!(params.aligned_rays_per_probe(), 64);
    }

    #[test]
    fn scrolling_wraps_toroidally_and_clears_on_large_jump() {
        let spacing = [2.0, 2.0, 2.0];
        let counts = [8, 4, 8];
        let mut state = DdgiScrollingState::new([0.0, 0.0, 0.0], spacing);
        let delta = state.update([4.0, 0.0, -2.0], spacing, counts);
        assert_eq!(delta.step, [2, 0, -1]);
        assert!(!delta.clear_all);
        assert_eq!(state.scroll_offsets, [2, 0, 7]);
        assert_eq!(state.logical_to_storage_index([7, 0, 2], counts), [1, 0, 1]);

        let jump = state.update([100.0, 0.0, 0.0], spacing, counts);
        assert!(jump.clear_all);
        assert_eq!(state.scroll_offsets, [0, 0, 0]);
    }

    #[test]
    fn variability_sleeps_after_settle_and_wakes_on_event() {
        let mut var = DdgiVariabilityState {
            settle_frames: 3,
            ..Default::default()
        };
        assert!(var.should_trace());
        var.record_frame(0.001);
        var.record_frame(0.002);
        assert!(var.should_trace());
        var.record_frame(0.001);
        assert!(!var.should_trace());
        var.wake();
        assert!(var.should_trace());
    }

    #[test]
    fn rtxgi_backend_falls_back_without_ray_query_or_tensor_cores() {
        let no_rt = RayTracingCaps::default();
        assert_eq!(
            select_rtxgi_backend(RtxgiBackend::FullSuite, &no_rt, true),
            RtxgiBackend::Off
        );

        let rt = RayTracingCaps {
            ray_query: true,
            acceleration_structure: true,
            buffer_device_address: true,
            ..Default::default()
        };
        assert_eq!(
            select_rtxgi_backend(RtxgiBackend::Nrc, &rt, false),
            RtxgiBackend::Sharc
        );
        assert_eq!(
            select_rtxgi_backend(RtxgiBackend::Nrc, &rt, true),
            RtxgiBackend::Nrc
        );
    }
}
