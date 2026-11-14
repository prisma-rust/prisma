//! Diagnostic and isolation tools for rendering anomalies.
//!
//! Provides knobs to isolate:
//! 1. Vulkan barriers & synchronization hazards (`PRISMREV_PESSIMISTIC_BARRIER=1`)
//! 2. Pass execution & bypass (`PRISMREV_BYPASS_PASSES=gtao,taa`)
//! 3. Temporal accumulation & jitter (`PRISMREV_FREEZE_TEMPORAL=1`)
//! 4. Material diagnostic overrides (`PRISMREV_MATERIAL_OVERRIDE=normal|depth|...`)
//! 5. Geometry draw call bisection (`PRISMREV_DEBUG_DRAW_RANGE=0..10`)

/// Material override modes for diagnostic shading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialOverride {
    Normal,
    Depth,
    Direct,
    Shadow,
    Specular,
    Metallic,
    Roughness,
    DiffuseIbl,
    SpecularIbl,
    AmbientOcclusion,
    Emissive,
    Clay,
}

impl MaterialOverride {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "normal" | "normals" => Some(Self::Normal),
            "depth" => Some(Self::Depth),
            "direct" => Some(Self::Direct),
            "shadow" | "shadows" => Some(Self::Shadow),
            "specular" => Some(Self::Specular),
            "metallic" => Some(Self::Metallic),
            "roughness" => Some(Self::Roughness),
            "diffuse_ibl" | "diffuse-ibl" | "ibl_diffuse" => Some(Self::DiffuseIbl),
            "specular_ibl" | "specular-ibl" | "ibl_specular" => Some(Self::SpecularIbl),
            "ao" | "gtao" | "ambient_occlusion" => Some(Self::AmbientOcclusion),
            "emissive" => Some(Self::Emissive),
            "clay" => Some(Self::Clay),
            _ => None,
        }
    }
}

/// Rendering diagnostic control state.
#[derive(Debug, Clone, Default)]
pub struct RenderDebugControl {
    /// Injects full execution and memory barriers around every render pass.
    pub pessimistic_barrier: bool,
    /// List of pass names (lowercase substring) to bypass in the RenderGraph.
    pub bypassed_passes: Vec<String>,
    /// Freezes temporal accumulation and history buffers.
    pub freeze_temporal: bool,
    /// Material shading override mode.
    pub material_override: Option<MaterialOverride>,
    /// Slices draw items to a specific range (start..end).
    pub draw_range: Option<(usize, usize)>,
}

impl RenderDebugControl {
    pub fn from_env() -> Self {
        let pessimistic_barrier = std::env::var("PRISMREV_PESSIMISTIC_BARRIER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let bypassed_passes = std::env::var("PRISMREV_BYPASS_PASSES")
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_ascii_lowercase())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let freeze_temporal = std::env::var("PRISMREV_FREEZE_TEMPORAL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let material_override = std::env::var("PRISMREV_MATERIAL_OVERRIDE")
            .ok()
            .and_then(|s| MaterialOverride::parse(&s));

        let draw_range = std::env::var("PRISMREV_DEBUG_DRAW_RANGE")
            .ok()
            .and_then(|s| {
                let s = s.trim();
                let (start, end) = s.split_once("..")?;
                let start = start.trim().parse::<usize>().ok()?;
                let end = end.trim().parse::<usize>().ok()?;
                Some((start, end))
            });

        Self {
            pessimistic_barrier,
            bypassed_passes,
            freeze_temporal,
            material_override,
            draw_range,
        }
    }

    /// Check whether a pass with the given name should be bypassed.
    pub fn is_pass_bypassed(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.bypassed_passes.iter().any(|b| lower.contains(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_material_override_parse() {
        assert_eq!(
            MaterialOverride::parse("normal"),
            Some(MaterialOverride::Normal)
        );
        assert_eq!(
            MaterialOverride::parse("depth"),
            Some(MaterialOverride::Depth)
        );
        assert_eq!(
            MaterialOverride::parse("clay"),
            Some(MaterialOverride::Clay)
        );
        assert_eq!(
            MaterialOverride::parse("ao"),
            Some(MaterialOverride::AmbientOcclusion)
        );
        assert_eq!(MaterialOverride::parse("invalid"), None);
    }

    #[test]
    fn test_is_pass_bypassed() {
        let control = RenderDebugControl {
            bypassed_passes: vec!["gtao".into(), "taa".into()],
            ..Default::default()
        };
        assert!(control.is_pass_bypassed("GtaoPass"));
        assert!(control.is_pass_bypassed("gtao"));
        assert!(control.is_pass_bypassed("TaaPass"));
        assert!(!control.is_pass_bypassed("ForwardPass"));
        assert!(!control.is_pass_bypassed("PostPass"));
    }
}
