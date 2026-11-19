//! Embedded compiled SPIR-V bytecode slices (`assets/shaders/*.spv`).
//!
//! Kept in a leaf crate with zero dependencies so `.spv` edits do not invalidate
//! `prism-render`'s 58k-line compilation unit when bindings are unchanged.

pub const ARM_ASR_UPSCALE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/arm_asr_upscale.comp.spv");
pub const AUTO_EXPOSURE_ADAPT_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/auto_exposure_adapt.comp.spv");
pub const AUTO_EXPOSURE_HIST_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/auto_exposure_hist.comp.spv");
pub const DDGI_BLEND_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ddgi_blend.comp.spv");
pub const DDGI_CLEAR_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ddgi_clear.comp.spv");
pub const DDGI_RESOLVE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ddgi_resolve.comp.spv");
pub const DDGI_UPDATE_DISTANCE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ddgi_update_distance.comp.spv");
pub const DDGI_UPDATE_IRRADIANCE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ddgi_update_irradiance.comp.spv");
pub const GI_BAKE_COMP_SPV: &[u8] = include_bytes!("../../../assets/shaders/gi_bake.comp.spv");
pub const GIZMO_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/gizmo.frag.spv");
pub const GIZMO_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/gizmo.vert.spv");
pub const GTAO_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/gtao.frag.spv");
pub const GTAO_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/gtao.vert.spv");
pub const HARNESS_BENCH_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/harness_bench.comp.spv");
pub const HARNESS_COPY_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/harness_copy.comp.spv");
pub const IBL_BRDF_LUT_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ibl_brdf_lut.comp.spv");
pub const IBL_EQUIRECT_TO_CUBE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ibl_equirect_to_cube.comp.spv");
pub const IBL_IRRADIANCE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ibl_irradiance.comp.spv");
pub const IBL_PREFILTER_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ibl_prefilter.comp.spv");
pub const MESH_VERT_SKINNED_VERT_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/mesh_vert_skinned.vert.spv");
pub const MESH_VERT_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/mesh_vert.vert.spv");
pub const OUTLINE_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/outline.frag.spv");
pub const OUTLINE_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/outline.vert.spv");
pub const PARTICLES_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/particles.frag.spv");
pub const PARTICLES_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/particles.vert.spv");
pub const POST_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/post.frag.spv");
pub const POST_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/post.vert.spv");
pub const PRT_CLUSTER_EVAL_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/prt_cluster_eval.comp.spv");
pub const PRT_PROPAGATE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/prt_propagate.comp.spv");
pub const PRT_RESOLVE_VOLUME_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/prt_resolve_volume.comp.spv");
pub const PT_REFERENCE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/pt_reference.comp.spv");
pub const PT_RENDER_COMP_SPV: &[u8] = include_bytes!("../../../assets/shaders/pt_render.comp.spv");
pub const PT_SHARC_TRAIN_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/pt_sharc_train.comp.spv");
pub const RESTIR_GI_COMP_SPV: &[u8] = include_bytes!("../../../assets/shaders/restir_gi.comp.spv");
pub const RT_PREVIEW_FRAG_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/rt_preview.frag.spv");
pub const RT_PREVIEW_VERT_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/rt_preview.vert.spv");
pub const RT_RENDER_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/rt_render.frag.spv");
pub const RT_RENDER_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/rt_render.vert.spv");
pub const SCENE_FRAG_FRAG_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/scene_frag.frag.spv");
pub const SGSR2_UPSCALE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/sgsr2_upscale.comp.spv");
pub const SHADOW_DEPTH_FRAG_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/shadow_depth.frag.spv");
pub const SHADOW_DEPTH_SKINNED_VERT_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/shadow_depth_skinned.vert.spv");
pub const SHADOW_DEPTH_VERT_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/shadow_depth.vert.spv");
pub const SHARC_RESOLVE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/sharc_resolve.comp.spv");
pub const SKYBOX_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/skybox.frag.spv");
pub const SKYBOX_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/skybox.vert.spv");
pub const SPRITES_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/sprites.frag.spv");
pub const SPRITES_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/sprites.vert.spv");
pub const TAA_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/taa.frag.spv");
pub const TAA_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/taa.vert.spv");
pub const UI_OVERLAY_FRAG_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ui_overlay.frag.spv");
pub const UI_OVERLAY_VERT_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/ui_overlay.vert.spv");
pub const VOXEL_COMPOSITE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/voxel_composite.comp.spv");
pub const VOXEL_DDA_COMP_SPV: &[u8] = include_bytes!("../../../assets/shaders/voxel_dda.comp.spv");
pub const VOXEL_GI_DENOISE_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/voxel_gi_denoise.comp.spv");
pub const VOXEL_RT_GI_COMP_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/voxel_rt_gi.comp.spv");
