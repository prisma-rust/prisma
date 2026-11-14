//! Unified RTXGI Suite coordinator (DDGI, SHaRC, NRC).

/// Active RTXGI GI backend mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtxgiBackend {
    Off,
    Ddgi,
    Sharc,
    Nrc,
    FullSuite,
}

/// DDGI probe-volume configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DdgiConfig {
    pub probe_counts: [u32; 3],
    pub probe_spacing: f32,
    pub hysteresis: f32,
    pub rays_per_probe: u32,
}

impl Default for DdgiConfig {
    fn default() -> Self {
        Self {
            probe_counts: [16, 8, 16],
            probe_spacing: 2.0,
            hysteresis: 0.97,
            rays_per_probe: 128,
        }
    }
}

/// SHaRC spatial hash radiance cache configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharcConfig {
    pub capacity: u32,
    pub accumulation_frames: u32,
    pub stale_frame_limit: u32,
}

impl Default for SharcConfig {
    fn default() -> Self {
        Self {
            capacity: 1 << 20,
            accumulation_frames: 8,
            stale_frame_limit: 64,
        }
    }
}

/// Neural Radiance Cache (NRC) configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NrcConfig {
    pub training_rays_per_frame: u32,
    pub learning_rate: f32,
    pub hidden_width: u32,
}

impl Default for NrcConfig {
    fn default() -> Self {
        Self {
            training_rays_per_frame: 16_384,
            learning_rate: 1e-3,
            hidden_width: 64,
        }
    }
}

/// Unified RTXGI Suite state & pass-routing coordinator.
#[derive(Clone, Debug)]
pub struct RtxgiSuiteState {
    pub backend: RtxgiBackend,
    pub ddgi: DdgiConfig,
    pub sharc: SharcConfig,
    pub nrc: NrcConfig,
    pub frame_index: u64,
}

impl Default for RtxgiSuiteState {
    fn default() -> Self {
        Self {
            backend: RtxgiBackend::FullSuite,
            ddgi: DdgiConfig::default(),
            sharc: SharcConfig::default(),
            nrc: NrcConfig::default(),
            frame_index: 0,
        }
    }
}

impl RtxgiSuiteState {
    pub fn ddgi_enabled(&self) -> bool {
        matches!(self.backend, RtxgiBackend::Ddgi | RtxgiBackend::FullSuite)
    }

    pub fn sharc_enabled(&self) -> bool {
        matches!(self.backend, RtxgiBackend::Sharc | RtxgiBackend::FullSuite)
    }

    pub fn nrc_enabled(&self) -> bool {
        matches!(self.backend, RtxgiBackend::Nrc | RtxgiBackend::FullSuite)
    }

    pub fn advance_frame(&mut self) {
        self.frame_index = self.frame_index.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_suite_enables_all_three_backends() {
        let mut state = RtxgiSuiteState::default();
        assert!(state.ddgi_enabled());
        assert!(state.sharc_enabled());
        assert!(state.nrc_enabled());
        state.advance_frame();
        assert_eq!(state.frame_index, 1);

        state.backend = RtxgiBackend::Sharc;
        assert!(!state.ddgi_enabled());
        assert!(state.sharc_enabled());
        assert!(!state.nrc_enabled());
    }
}
