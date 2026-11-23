use super::*;
#[test]
fn push_constant_size() {
    assert_eq!(
        std::mem::size_of::<shader_bindings::pt_render::PtPush>(),
        176,
        "shader_bindings::pt_render::PtPush (repr(C))"
    );
    assert_eq!(
        PT_PUSH_RANGE_SIZE as usize,
        std::mem::size_of::<shader_bindings::pt_render::PtPush>(),
        "PT_PUSH_RANGE_SIZE must match the reflected push block"
    );
    assert_eq!(
        PT_PUSH_RANGE_SIZE,
        shader_bindings::pt_render::PUSH_CONSTANT_SIZE,
        "PT_PUSH_RANGE_SIZE must match the shader's reflected block size"
    );
}

/// `params.w` bit layout must match the `PT_PARAMS_*` constants in
/// `pt_render.slang`: bits 0..=29 frame counter, bit 30 orthographic camera,
/// bit 31 reset accumulation.
#[test]
fn pt_params_packing_matches_shader_bits() {
    assert_eq!(PT_PARAMS_FRAME_MASK, 0x3FFF_FFFF);
    assert_eq!(PT_PARAMS_ORTHOGRAPHIC, 1 << 30);
    assert_eq!(PT_PARAMS_RESET, 1 << 31);

    assert_eq!(pack_pt_params(0, false, false), 0);
    assert_eq!(pack_pt_params(42, false, false), 42);
    assert_eq!(pack_pt_params(42, true, false), 42 | (1 << 31));
    assert_eq!(pack_pt_params(42, false, true), 42 | (1 << 30));
    assert_eq!(pack_pt_params(42, true, true), 42 | (1 << 30) | (1 << 31));

    assert_eq!(pack_pt_params(u32::MAX, false, false), PT_PARAMS_FRAME_MASK);
}

/// `sharc_flags` bit layout must match `pt_render.slang`: bit 0 queries the
/// cache, bit 1 deposits, bits 8..=15 carry the deposit stride. A stride of
/// zero would silently stop every deposit.
#[test]
fn sharc_flags_pack_query_update_and_stride() {
    assert_eq!(SHARC_FLAG_QUERY, 1);
    assert_eq!(SHARC_FLAG_UPDATE, 1 << 1);
    assert_eq!(SHARC_FLAG_STRIDE_SHIFT, 8);

    assert_eq!(pack_sharc_flags(false, false, 1), 1 << 8);
    assert_eq!(pack_sharc_flags(true, false, 1), 0x101);
    assert_eq!(pack_sharc_flags(true, true, 1), 0x103);
    assert_eq!(pack_sharc_flags(true, true, 5), 0x503);

    assert_eq!(pack_sharc_flags(false, true, 5), 5 << 8);

    assert_eq!(pack_sharc_flags(false, false, 0) >> 8, 1);
    assert_eq!(pack_sharc_flags(false, false, u32::MAX) >> 8, 0xFF);
    assert_eq!(pack_sharc_flags(true, true, u32::MAX), 0xFF03);
}

#[test]
fn transport_variants_share_the_push_and_entry_contract() {
    use shader_bindings::{pt_reference, pt_render, pt_sharc_train};

    for spv in [
        variant_spirv(PtVariant::Consume),
        variant_spirv(PtVariant::Reference),
        variant_spirv(PtVariant::Train),
    ] {
        assert!(!spv.is_empty(), "variant SPIR-V is embedded");
        assert_eq!(&spv[..4], &[0x03, 0x02, 0x23, 0x07], "SPIR-V magic");
    }

    assert_eq!(
        std::mem::size_of::<pt_reference::PtPush>(),
        std::mem::size_of::<pt_render::PtPush>()
    );
    assert_eq!(
        std::mem::size_of::<pt_sharc_train::PtPush>(),
        std::mem::size_of::<pt_render::PtPush>()
    );
    assert_eq!(
        pt_reference::PUSH_CONSTANT_SIZE,
        pt_render::PUSH_CONSTANT_SIZE
    );
    assert_eq!(
        pt_sharc_train::PUSH_CONSTANT_SIZE,
        pt_render::PUSH_CONSTANT_SIZE
    );
    assert_eq!(pt_reference::ENTRY_PT_MAIN, pt_render::ENTRY_PT_MAIN);
    assert_eq!(pt_sharc_train::ENTRY_PT_MAIN, pt_render::ENTRY_PT_MAIN);
    assert_eq!(pt_render::PUSH_CONSTANT_SIZE, PT_PUSH_RANGE_SIZE);
}

/// The guide UBO and the guide storage images are a contract with
/// `pt_render.slang`: the shader indexes them by the reflected binding
/// numbers, and the std140 block must stay three 4x4 matrices wide. A size
/// mismatch here shows up as garbage matrices (wrong motion vectors and
/// normals), not as a compile error.
#[test]
fn guide_contract_matches_the_reflected_bindings() {
    use shader_bindings::{pt_reference, pt_render, pt_sharc_train};

    assert_eq!(std::mem::size_of::<PtGuides>(), 192);

    assert_eq!(pt_render::GUIDE_DEPTH_BINDING, 15);
    assert_eq!(pt_render::GUIDE_VELOCITY_BINDING, 16);
    assert_eq!(pt_render::GUIDES_BINDING, 17);
    assert_eq!(pt_render::GUIDE_NORMAL_ROUGHNESS_BINDING, 18);

    assert_eq!(
        pt_reference::GUIDE_DEPTH_BINDING,
        pt_render::GUIDE_DEPTH_BINDING
    );
    assert_eq!(
        pt_sharc_train::GUIDE_DEPTH_BINDING,
        pt_render::GUIDE_DEPTH_BINDING
    );
    assert_eq!(
        pt_reference::GUIDE_NORMAL_ROUGHNESS_BINDING,
        pt_render::GUIDE_NORMAL_ROUGHNESS_BINDING
    );
    assert_eq!(
        pt_sharc_train::GUIDE_NORMAL_ROUGHNESS_BINDING,
        pt_render::GUIDE_NORMAL_ROUGHNESS_BINDING
    );
}

#[test]
fn default_transport_variant_is_the_consumer() {
    assert_eq!(PtVariant::default(), PtVariant::Consume);
}

#[test]
fn training_extent_covers_the_edge_block() {
    let full = vk::Extent2D {
        width: 1920,
        height: 1080,
    };
    let half = PathTracePass::training_extent(full, 5);
    assert_eq!((half.width, half.height), (384, 216));
    let odd = vk::Extent2D {
        width: 1919,
        height: 1079,
    };
    let odd_train = PathTracePass::training_extent(odd, 5);
    assert_eq!((odd_train.width, odd_train.height), (384, 216));
    assert_eq!(
        PathTracePass::training_extent(full, 0),
        vk::Extent2D {
            width: 1920,
            height: 1080
        }
    );
}
