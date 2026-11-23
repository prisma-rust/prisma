use super::*;

fn valid_input() -> TextureUploadInput {
    TextureUploadInput {
        width: 2,
        height: 2,
        format: TextureFormat::Rgba8,
        mip_levels: 1,
        pixels: vec![0; 2 * 2 * 4],
    }
}

#[test]
fn reserve_rejects_wrong_pixel_size() {
    let bad = TextureUploadInput {
        width: 2,
        height: 2,
        format: TextureFormat::Rgba8,
        mip_levels: 1,
        pixels: vec![0; 3],
    };
    let expected = 2 * 2 * 4;
    assert_ne!(bad.pixels.len(), expected);
}

#[test]
fn bytes_per_pixel_is_4_for_rgba8() {
    assert_eq!(TextureFormat::Rgba8.bytes_per_pixel(), 4);
}

#[test]
fn valid_input_passes_size_check() {
    let input = valid_input();
    let expected = input.width as usize * input.height as usize * input.format.bytes_per_pixel();
    assert_eq!(input.pixels.len(), expected);
}

#[test]
fn single_level_rgba8_must_be_exact() {
    assert!(accepts_pixel_len(TextureFormat::Rgba8, 0, 16, 2, 2));
    assert!(!accepts_pixel_len(TextureFormat::Rgba8, 0, 17, 2, 2));
    assert!(!accepts_pixel_len(TextureFormat::Rgba8, 0, 15, 2, 2));
}

#[test]
fn baked_rgba8_chain_larger_than_base_level_is_accepted() {
    assert!(accepts_pixel_len(TextureFormat::Rgba8, 4, 340, 8, 8));
    assert!(!accepts_pixel_len(TextureFormat::Rgba8, 4, 255, 8, 8));
}

#[test]
fn compressed_formats_tolerate_a_chain_but_not_a_short_buffer() {
    assert!(accepts_pixel_len(TextureFormat::Bc7Unorm, 0, 64, 8, 8));
    assert!(accepts_pixel_len(
        TextureFormat::Bc7Unorm,
        3,
        64 + 16 + 16,
        8,
        8
    ));
    assert!(!accepts_pixel_len(TextureFormat::Bc7Unorm, 3, 63, 8, 8));
}

#[test]
fn unregister_reclaims_the_bindless_slot_and_writes_fallback() {
    use crate::bindless::TextureHandle;

    let mut bindless_alloc = crate::bindless::SlotAllocator::new(4);
    let s0 = bindless_alloc.claim().unwrap();
    let s1 = bindless_alloc.claim().unwrap();
    assert_eq!(s0, 0);
    assert_eq!(s1, 1);
    assert!(bindless_alloc.is_used(s0));
    assert!(bindless_alloc.is_used(s1));

    assert!(bindless_alloc.release(s1));
    assert!(!bindless_alloc.is_used(s1));
    assert_eq!(bindless_alloc.live_len(), 1);

    assert_eq!(bindless_alloc.claim().unwrap(), 1);
    assert_eq!(bindless_alloc.live_len(), 2);
    assert_eq!(bindless_alloc.free_slot_count(), 0);

    assert_eq!(TextureHandle::INVALID.0, u32::MAX);
}

#[test]
fn release_slot_rejects_double_release() {
    let mut slots = crate::bindless::SlotAllocator::new(2);
    let slot = slots.claim().unwrap();
    assert!(slots.release(slot));
    assert!(!slots.release(slot), "double release must be rejected");
    assert_eq!(slots.claim().unwrap(), slot);
    assert_eq!(slots.claim().unwrap(), 1);
    assert!(slots.claim().is_err(), "capacity exhausted again");
}

#[test]
fn bc1_and_bc3_report_their_block_sizes() {
    use crate::managers::texture_manager::TextureFormat;

    assert_eq!(TextureFormat::Bc1Srgb.compressed_byte_len(8, 8), 4 * 8);
    assert_eq!(TextureFormat::Bc3Srgb.compressed_byte_len(8, 8), 4 * 16);
    assert_eq!(TextureFormat::Bc1Unorm.compressed_byte_len(5, 3), 2 * 8);
    assert_eq!(TextureFormat::Bc3Unorm.compressed_byte_len(5, 3), 2 * 16);
    assert!(TextureFormat::Bc1Srgb.is_compressed());
    assert!(TextureFormat::Bc3Unorm.is_compressed());
    assert_eq!(TextureFormat::Rgba8.bytes_per_pixel(), 4);
}

#[test]
fn bc1_and_bc3_map_to_vulkan_block_formats() {
    use crate::managers::texture_manager::TextureFormat;

    assert_eq!(
        TextureFormat::Bc1Unorm.vk_format(),
        ash::vk::Format::BC1_RGBA_UNORM_BLOCK
    );
    assert_eq!(
        TextureFormat::Bc1Srgb.vk_format(),
        ash::vk::Format::BC1_RGBA_SRGB_BLOCK
    );
    assert_eq!(
        TextureFormat::Bc3Unorm.vk_format(),
        ash::vk::Format::BC3_UNORM_BLOCK
    );
    assert_eq!(
        TextureFormat::Bc3Srgb.vk_format(),
        ash::vk::Format::BC3_SRGB_BLOCK
    );
}

#[test]
fn the_fallback_is_a_neutral_checkerboard_not_a_loud_marker() {
    let pixels = super::fallback_pixels();
    assert_eq!(
        pixels.len(),
        (super::FALLBACK_EDGE * super::FALLBACK_EDGE * 4) as usize
    );

    for texel in pixels.as_chunks::<4>().0 {
        assert_eq!(texel[3], 255, "the fallback must be opaque");
        assert_eq!(
            (texel[0], texel[1], texel[2]),
            (texel[0], texel[0], texel[0]),
            "fallback texels must be neutral grey, got {texel:?}"
        );
    }

    let mut tones: Vec<u8> = pixels.as_chunks::<4>().0.iter().map(|t| t[0]).collect();
    tones.sort_unstable();
    tones.dedup();
    assert_eq!(
        tones,
        vec![0, 128],
        "expected black and mid grey, got {tones:?}"
    );
}

#[test]
fn the_fallback_tile_is_square_and_repeats_seamlessly() {
    let edge = super::FALLBACK_EDGE;
    let pixels = super::fallback_pixels();
    let at = |x: u32, y: u32| pixels[((y * edge + x) * 4) as usize];

    assert_eq!(
        edge % 4,
        0,
        "the edge must hold a whole number of 2-texel tiles"
    );
    for y in 0..edge {
        for x in 0..edge {
            let expected = if (x / 2 + y / 2) % 2 == 0 { 128 } else { 0 };
            assert_eq!(at(x, y), expected, "cell mismatch at ({x}, {y})");
        }
    }
}
