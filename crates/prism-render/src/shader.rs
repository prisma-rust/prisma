//! Shader module loading from SPIR-V bytecode.
//!
//! SPIR-V is compiled offline from Slang via `slangc` (see
//! `assets/shaders/compile.sh`) and embedded at compile time with
//! `include_bytes!`; `build.rs` turns missing `.spv` into one actionable message
//! (and compiles them when a `slangc` is available).
//!
//! Every module here is a built-in engine pass: descriptor bindings and
//! push-constant layouts come from the generated `shader_bindings`, so bytecode
//! and Rust pipeline code have to ship together. Project shaders are data:
//! `prism-asset` decodes a `ShaderAsset`, `GpuAssetResolver` resolves the
//! component's `shader_path`, and `GraphRenderer::register_custom_shader`
//! registers the bytecode at runtime.

use anyhow::Context as _;
use ash::vk;

/// Opaque handle to a registered custom fragment shader module in [`ForwardPass`](crate::ForwardPass).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderHandle(pub u32);

/// Load a shader module from SPIR-V bytecode already in memory.
///
/// The byte slice does **not** need to be 4-byte aligned; a temporary copy is
/// made if necessary.
pub fn load_shader_module(device: &ash::Device, code: &[u8]) -> anyhow::Result<vk::ShaderModule> {
    assert!(
        code.len().is_multiple_of(4),
        "SPIR-V bytecode length ({}) must be a multiple of 4",
        code.len()
    );

    let words: Vec<u32> = if (code.as_ptr() as usize).is_multiple_of(4) {
        let words =
            unsafe { std::slice::from_raw_parts(code.as_ptr() as *const u32, code.len() / 4) };
        words.to_vec()
    } else {
        code.as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_ne_bytes(*chunk))
            .collect()
    };

    let create_info = vk::ShaderModuleCreateInfo::default().code(&words);
    let module = unsafe { device.create_shader_module(&create_info, None) }
        .context("create shader module")?;
    Ok(module)
}

/// Build a `VkPipelineShaderStageCreateInfo` from a shader module and entry
/// point name (as `&CStr`).
///
/// The caller must ensure the `CStr` lives as long as the returned info
/// (ash stores a raw pointer). Entry-point names come from Slang reflection
/// (e.g. `vertexMain`/`fragmentMain`); see `shader_bindings`.
pub fn shader_stage<'a>(
    stage: vk::ShaderStageFlags,
    module: vk::ShaderModule,
    entry_point: &'a std::ffi::CStr,
) -> vk::PipelineShaderStageCreateInfo<'a> {
    vk::PipelineShaderStageCreateInfo::default()
        .stage(stage)
        .module(module)
        .name(entry_point)
}
