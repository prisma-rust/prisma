//! Unified dirty dispatch plan (`DirtyDispatchPlan`, DESIGN §9 / PR-S2).
//!
//! In `prepare` phase, consume scene and asset dirty flags once, batch in topological order:
//! 1. Texture upload / release (`RenderTextureManager`)
//! 2. Mesh upload / BLAS rebuild (`RenderMeshManager`)
//! 3. Material buffer update (`RenderMaterialManager`)
//! 4. Light source buffer update (`RenderAnalyticLightManager`)
//! 5. Instance / draw item changes (`RenderInstanceManager`)
//! 6. Skybox and emissive table cascade update (`RenderSkyManager` / `RenderEmissiveLightTable`)

/// Unified dispatch plan for scene and resource changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtyDispatchPlan {
    /// Number of textures pending upload / update
    pub texture_uploads: usize,
    /// Number of texture slots pending release
    pub texture_releases: usize,
    /// Number of meshes pending upload / rebuild
    pub mesh_uploads: usize,
    /// Number of mesh handles pending release
    pub mesh_releases: usize,
    /// Number of materials pending upload / update
    pub material_updates: usize,
    /// Number of material slots pending release
    pub material_releases: usize,
    /// Whether camera changed (VP/Eye/Projection)
    pub camera_dirty: bool,
    /// Whether directional or local light sources changed
    pub lights_dirty: bool,
    /// Whether skybox / IBL changed
    pub sky_dirty: bool,
    /// Whether instance / draw list content changed
    pub instances_dirty: bool,
    /// Whether cascade emissive mesh table needs rebuild (triggered by mesh/material/instances dirty cascade)
    pub emissive_table_dirty: bool,
    /// Whether path tracing accumulation needs reset
    pub pt_accum_dirty: bool,
}

impl DirtyDispatchPlan {
    /// Check whether any resource upload or state update needs dispatch.
    pub fn has_work(&self) -> bool {
        self.texture_uploads > 0
            || self.texture_releases > 0
            || self.mesh_uploads > 0
            || self.mesh_releases > 0
            || self.material_updates > 0
            || self.material_releases > 0
            || self.camera_dirty
            || self.lights_dirty
            || self.sky_dirty
            || self.instances_dirty
            || self.emissive_table_dirty
            || self.pt_accum_dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_plan_has_no_work() {
        let plan = DirtyDispatchPlan::default();
        assert!(!plan.has_work());
    }

    #[test]
    fn plan_detects_individual_work_items() {
        let plan = DirtyDispatchPlan {
            texture_uploads: 1,
            ..Default::default()
        };
        assert!(plan.has_work());

        let plan2 = DirtyDispatchPlan {
            camera_dirty: true,
            ..Default::default()
        };
        assert!(plan2.has_work());

        let plan3 = DirtyDispatchPlan {
            pt_accum_dirty: true,
            ..Default::default()
        };
        assert!(plan3.has_work());
    }
}
