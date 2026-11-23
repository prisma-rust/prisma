# Vulkan render loop lessons learned

Pitfalls encountered and lessons learned while implementing Prisma milestone 1 (desktop Vulkan clear-screen loop).

## 1. Broken toolchain must be thoroughly cleaned

**Problem**: `rustup` state corrupted (manifest missing, download cache polluted, components marked installed but binaries missing).

**Lessons**:
- `rustup toolchain install` showing "unchanged" doesn't mean toolchain intact, only manifest thinks installed
- Git Bash `rm -rf` on Windows may fail to delete locked files, use `cmd //c "rmdir /s /q ..."`
- Repair steps: first `rustup toolchain uninstall`, then clear `~/.rustup/downloads` and `tmp`, finally reinstall
- Proxy settings: `HTTP_PROXY/HTTPS_PROXY=http://127.0.0.1:7897`, rustup default backend (reqwest) goes through HTTP CONNECT tunnel

## 2. ash 0.38 API differs greatly from old versions

**Problem**: many ash API calls written from memory failed to compile.

**Lessons**:
- **Never guess API**, read ash source under `~/.cargo/registry/src/` to confirm signatures
- Key differences:
  - `DebugUtilsObjectNameInfoEXT` `object_type` is a field not builder method; `object_handle<T: Handle>()` is generic method
  - `set_debug_utils_object_name` is on `ash::ext::debug_utils::Device`, not `Instance`
  - Extension-name constants (e.g. `vk::EXT_DEBUG_UTILS_NAME`) are `&'static CStr`, not `&str`, can't pass directly to `CString::new`
  - `ash_window::create_surface` takes `RawDisplayHandle`/`RawWindowHandle` (values), needs `.into()` from `HasDisplayHandle`
  - `ash_window::enumerate_required_extensions` returns `&'static [*const c_char]`

## 3. Device creation must enable swapchain extension

**Problem**: `Unable to load create_swapchain_khr` panic.

**Cause**: didn't enable `VK_KHR_swapchain` device extension when creating logical device, swapchain function pointers couldn't load.

**Lesson**: instance extensions and device extensions are different things. `VK_KHR_swapchain` is a **device extension**, must be enabled in `vk::DeviceCreateInfo::enabled_extension_names`.

## 4. Swapchain recreation must pass old_swapchain

**Problem**: `VK_ERROR_NATIVE_WINDOW_IN_USE_KHR` on swapchain recreation.

**Cause**: `vk::SwapchainCreateInfoKHR` didn't set `old_swapchain`, implementation couldn't smoothly transition old swapchain.

**Lessons**:
- Must pass old swapchain to `create_info.old_swapchain(old)` on recreation
- Destroy old only after new swapchain created successfully
- Recreation transactional: create new first, destroy old resources (view / semaphore) only on success, keep old state unchanged on failure

## 5. Semaphore sync: per-image not per-frame

**Problem**: validation layer reports semaphore reuse error `VUID-vkQueueSubmit-pSignalSemaphores-00067`.

**Cause**: `render_finished` semaphore rotated by frame-index, but `vkAcquireNextImageKHR` may return same image index consecutively, causing same semaphore referenced by two presents.

**Lessons**:
- `image_available` (acquire semaphore): rotate by `MAX_FRAMES_IN_FLIGHT`, fence guarantees safe reuse
- `render_finished` (semaphore waited by present): **indexed by swapchain image index**, one per image
- Follow validation-layer advice directly: `Use a separate semaphore per swapchain image`
- Editor window swapchain managed by `egui-ash`, upstream rev `258aee2` still per-frame; #581 fixed via
  `prisma-rust/egui-ash` `prismrev` branch (commit `6b390d3`) before integration,
  when upgrading this dependency later reconfirm `src/presenters.rs` still takes semaphore by `image_index`

## 6. Command buffer must be per-frame-in-flight

**Problem**: `vkResetCommandBuffer` reports "commandBuffer must not be in the pending state".

**Cause**: only one command buffer shared by all frames. Frame A submitted (pending), frame B reset it without waiting for A.

**Lessons**:
- Allocate `MAX_FRAMES_IN_FLIGHT` command buffers, index by `current_frame`
- Fence wait ensures corresponding frame's command buffer finished before reset and re-record

## 7. Frame sync: don't use image_in_flight tracking

**Problem**: acquire blocks forever at frame 3.

**Cause**: `image_in_flight[image_index]` set to submit fence, but whether image reusable depends on **present completion**, not submit. `wait_for_fences(image_in_flight[idx])` waits submit fence, can't guarantee image released by present, forming circular wait.

**Lessons**:
- Use vulkan-tutorial standard pattern: only `MAX_FRAMES_IN_FLIGHT` fences rotating, **no per-image fence tracking needed**
- `MAX_FRAMES_IN_FLIGHT=2` + `min_image_count+1=3` swapchain images, always at least 1 free image, acquire won't block
- Fence wait at acquire start: `wait_for_fences([current_frame_fence])` -> `reset_fences` -> `acquire`, ensuring command buffer and acquire semaphore safely reusable

## 8. winit 0.30 event-loop model

**Problem**: double-clicking run window "not responding", color doesn't change.

**Cause**: winit 0.30 event-driven, `about_to_wait` doesn't auto-call continuously.

**Lessons**:
- Must call `window.request_redraw()` to trigger next frame
- Put `request_redraw` in `about_to_wait` (not in `RedrawRequested` callback), letting event loop return to OS message pump between frames
- Render logic in `window_event` `RedrawRequested` branch
- On Windows FIFO present needs message pump running to complete, non-breathing event loop causes images not released

## 9. Logging in no-console environment issue

**Problem**: double-clicking run `env_logger` writing stderr may block.

**Lessons**:
- Use `std::io::IsTerminal` to detect console presence
- Without console fall back to writing file (`template.log`), avoid blocking
- Add `buf.flush()` in log `format` closure to ensure real-time disk write for crash diagnosis

## 10. Matrix multiplication and column-major pitfalls

**Problem**: render loop runs but only clear color shows, no drawn objects (all discarded at clipping).

**Cause**: several matrix math calculations written wrong due to ignoring column-major (or typos), mapping object coords to invalid clip space:
1. **View-Projection multiplication reversed**: custom multiply `C[i][j] += A[i][k] * B[k][j]` actually computes `(B * A)^T`. Since `i` is column, `j` row, standard `C = A * B` should be `C[i][j] += A[k][j] * B[i][k]`.
2. **Projection Z/W element position reversed**: in column-major `proj[col][row]`, to realize `W_clip = -Z_view`, `-1.0` goes at `col2.w` (`proj[2][3]`), Z translation at `col3.z` (`proj[3][2]`). Original had them swapped, W got wrong scale.
3. **Model matrix local scale applied wrong**: `to_model_matrix` applied scale by multiplying a *row* with different sx/sy/sz. For `M = T * R * S`, scale applies in local coords, so each rotation matrix column vector (local X/Y/Z axis) should be multiplied by corresponding scale factor.

**Lessons**:
- **Always manually walk through column-major 2D-array indices on paper**. With `m[col][row]` doing `C = A * B`, result column `i` is `B`'s column `i` dotted with each row of `A`.
- Hand-writing low-level math in graphics APIs is error-prone, one index mistake causes black / empty screen. Adopt tested math library (`glam` / `nalgebra`) early; if must hand-write, rigorous unit tests for every matrix build / multiply.

## 11. Winding order and back-face culling empty-mask illusion

**Problem**: sphere lighting direction looks reversed. Light at upper-right but sphere seems lit upper-left. When camera moves to same side as light, sphere becomes all black, while nearby cube lit normally.

**Cause**: sphere triangle indices used **clockwise (CW)** winding, cube used **counter-clockwise (CCW)**.
1. **Coordinate system and front-face determination**: Vulkan projection flips Y (`proj[1][1] = -inv_tan`, correct Vulkan not bug). A triangle CCW in world space (+Y up) becomes CW in NDC after y-flip (Vulkan NDC y opposite OpenGL: y=+1 is framebuffer bottom, not top).
2. **Pipeline state**: graphics pipeline set `front_face(CLOCKWISE)` with `cull_mode(BACK)`. World-space CCW triangles (cube) become CW in clip space, judged front and kept; world-space CW triangles (sphere) judged back and culled.
3. **Visual illusion (hollow-mask illusion)**: front hemisphere facing camera entirely culled, camera actually sees **inside of back hemisphere**! Inner normals point outward (away from camera and light), making `dot(N, L)` counter-intuitive — reversed lighting or all-black illusion.

**Lessons**:
- **Unify winding order**: all generated 3D geometry (cube or sphere) must strictly follow unified winding (convention: world-space CCW = front face).
- **Weird lighting check normals and culling first**: when object lighting inconsistent or unreasonable pure black at angles, check winding / normal direction first, or temporarily disable back-face culling (`CullModeFlags::NONE`) to rule out rendering inner faces.

## Summary

Core of Vulkan render loop is **correct pairing of synchronization primitives**. Easiest mistakes aren't Vulkan complexity but sync design:

| Primitive | Count | Indexing | Purpose |
|------|------|----------|------|
| acquire semaphore | `MAX_FRAMES_IN_FLIGHT` | `current_frame` rotate | acquire -> submit wait |
| render_finished semaphore | swapchain image count | `image_index` | submit -> present wait |
| fence | `MAX_FRAMES_IN_FLIGHT` | `current_frame` rotate | CPU wait GPU command-buffer completion |

**Don't over-engineer**: standard pattern (fence rotation + per-image render_finished) is enough, no per-image fence tracking needed. Extra sync only introduces deadlocks.

## 12. Vulkan 1.2 below requires explicit enable and KHR wrapper for `VK_KHR_synchronization2`

**Problem**: `Unable to load cmd_pipeline_barrier2` non-unwind panic (crash at `vkCmdPipelineBarrier2` in `create_and_upload_image`).

**Causes**:
- Instance / device created with `API_VERSION_1_2`. On 1.2 core symbol `vkCmdPipelineBarrier2` **not in dispatch table**, only `vkCmdPipelineBarrier2KHR` available (entered core in 1.3).
- Only `enabled_extension_names.push(ash::khr::synchronization2::NAME)` **not enough** — `ash::Device::cmd_pipeline_barrier2` loads core symbol only, still panics.

**Lessons**:
- To use sync2 barrier on 1.2 device, must add extension to `enabled_extension_names` and use `ash::khr::synchronization2::Device::new(&instance, &device)` KHR wrapper to call.
- Same for `cmd_blit_image2` on 1.2 comes from `VK_KHR_copy_commands2`, also needs KHR wrapper.
- Don't try upgrading instance to 1.3 to "cheat" — physical device may only support 1.2, directly causing `createDevice` failure.

## 13. Slang bindless runtime arrays generate illegal SPIR-V

**Problem**: validation layer `vkCreateShaderModule()` reports `Invalid explicit layout decorations on type` (`%_runtimearr_XX ArrayStride 8`), VUID-StandaloneSpirv-None-10684.

**Cause**: `Texture2D[]` / `SamplerState[]` (`UniformConstant` runtime arrays) wrongly decorated by Slang with `OpDecorate <arr> ArrayStride 8`. SPIR-V validation forbids explicit layout decoration on "runtime arrays whose elements are opaque types (image/sampler)". Bug reproduces on all `spirv_1_3`~`spirv_1_6` profiles, **fixed since Slang 2026.21**.

**Lessons**:
- Must pass `spirv-val` after compile; such errors only exposed under debug validation, release doesn't validate but driver behavior undefined. Gate at end of `compile.sh` does this.
- 2025.x / 2026.13 outputs need stripping illegal `ArrayStride` (only "runtime arrays whose elements are `OpTypeImage`/`OpTypeSampler`", **legit strides of SSBO / struct arrays must be kept**). This post-processing (`fix_spirv.py`) removed after upgrading to Slang 2026.21; don't add back manually, upgrade compiler instead.
- Always prefer `.local/tools/slang/bin/slangc` (2026.21+) in any environment, `compile.sh` auto-detects. 2025.11 also had `scene_frag.slang` segfault, likewise only fixed by upgrade.

**Manual-compile trap**: don't bypass `compile.sh` to call `slangc` directly (especially old versions + `-emit-spirv-directly`), will emit spv with illegal `ArrayStride`. Always use `bash assets/shaders/compile.sh` (or `run.ps1`); after manual compile at least run `spirv-val.exe <file>.spv` once before commit.

## 14. Changed shader source but no effect? check three things first

**Problem**: repeatedly changed IBL constants in `scene_bindless.slang`, runtime unchanged ("still gray-white").

**Causes (check in order)**:
1. **Wrong shader filename / build script caused `.spv` never recompiled**: `compile.sh` has `set -euo pipefail`, a wrong shader source filename mid-script (e.g. `scene.slang` actually `scene.vert.slang`/`scene.frag.slang`) makes script `exit` early, **next extension shaders (incl. bindless) not regenerated**, disk still has old `.spv`. Profile name must match too: current slangc accepts `spirv_1_5`, rejects old `sspirv_1_5`.
2. **Changed wrong shader file**: sponza goes `GraphRenderer` → `ForwardPass` (`scene_bindless.frag.spv`) direct to swapchain. `GBufferPass`/`LightingPass`/`PostPass` currently **not connected by GraphRenderer**, changing `lighting.slang`/`post.slang` is wasted work.
3. **`include_bytes!` baked into binary at compile time**: must `cargo build` to recompile Rust to pick up new `.spv`.

**Lessons**:
- After change **must personally decompile `.spv` with `spirv-dis`, confirm relevant `OpConstant %float` actually changed**, don't just trust `compile.sh` printing "compiled".
- Before changing shader grep actual `include_bytes!` `FRAG_SPV`, and passes truly connected in `GraphRenderer::render` / `add_pass`, confirm changing binary actually executed.
- "Mask a segment to see what remains" is effective isolation (mask IBL, gray-white disappears → confirms IBL culprit), but only if isolation changed the right file.

## 15. Color space: SRGB swapchain double gamma-encodes output

**Problem**: unlit directly `return sampled.rgb` (texture sRGB bytes), red-brown tiles brightened to **yellow-white**.

**Cause**: swapchain is `B8G8R8A8_SRGB`, Vulkan applies another sRGB encode (gamma 1/2.2) to shader output before display. Texture uploaded as `R8G8B8A8_UNORM` (sRGB bytes), unlit outputs directly → double-encoded by swapchain → bright yellow-white.
**No post involved** (sponza path didn't connect PostPass at all), "yellow-white" can't be blamed on post `pow(1/2.2)` — wrong attribution.

**Lessons**:
- For unlit "what you see is texture": before output `albedo = pow(sampled.rgb, 2.2)` (sRGB→linear), letting SRGB swapchain encode back = original texture color.
- **Ambient "gray-white pollution" not operator problem**: diffuse IBL `irradiance * albedo` is multiply, correct. Real problem: `irradiance` is a **constant gray value** (e.g. 0.06–0.12 gray), added as independent term, leaving only gray-white on unlit side. Correct: sample real irradiance from `envCube` (with ambient tint), not a lump of gray constant.
**Texture formats should distinguish sRGB / linear (root fix, still TODO)**: albedo / emissive are sRGB data, should upload as `R8G8B8A8_SRGB` (sampling auto-converts to linear, lighting computed in linear, SRGB swapchain encodes back); normal / metallic-roughness are linear data, must stay `R8G8B8A8_UNORM`. Currently all textures uniformly `UNORM`, root defect of PBR color correctness and source of gamma hacks.

## Summary (supplement)

This milestone problems concentrated in **extension / validation / build chain / color space** four categories, not sync primitives:

| Category | Key pitfall | Verification |
|------|--------|----------|
| Extension loading | sync2/copy2 on 1.2 need explicit enable + KHR wrapper | crash stack points to `load_erased` |
| SPIR-V validity | Slang adds illegal `ArrayStride` to opaque runtime arrays | `spirv-val` reports VUID-StandaloneSpirv |
| Build chain | build script mid-failure leaves `.spv` stale; changed wrong file | `spirv-dis` decompile confirm constant |
| Color space | SRGB swapchain double gamma; IBL gray constant | unlit output + isolate IBL |
| Path confirmation | sponza uses ForwardPass direct swapchain, no post | grep `include_bytes!` + `add_pass` |

**Confirm actual path before changing code**: all "changed but no effect / wrong attribution" stem from preconceived assumptions about render path. Before changing shader confirm it's compiled, connected, binary-included.

## 16. Orthographic shadow projection z mapping and near plane must be self-consistent (else stripes / disappearance)

**Background**: `DirectionalLight` `light_direction` in shader is **direction TO the light** (`dot(n, light_dir)` lit, see `scene_bindless.slang` lines 453/455), orthographic shadow projection uses Vulkan `[0,1]` depth.

**Three pitfalls encountered (in order)**:

1. **Wrong z mapping formula → far saturates into stripes**: original `proj[2][2]=-0.5/dist, proj[2][3]=0.5` maps `view_z∈[-2*dist,0]` to `[1.5,0.5]`, far `clip.z>1` clipped / saturated to 1.0, ground shows regular stripes (shadow-acne-like). Correct Vulkan 0..1 orthographic:
   ```rust
   // view_z = -z (z is distance to light, ∈[n,f]); near=n, far=f
   proj[2][2] = -1.0/(f-n);  proj[2][3] = -n/(f-n);
   ```
2. **near = dist clips origin → shadow completely disappears**: `dist = half*2`, light at distance `dist` from origin, `near=dist` puts near plane at origin → origin and surroundings clipped before near → shadow map stays cleared 1.0 → `SampleCmpLevelZero` always returns lit. Must set `near` **less than** origin distance, e.g. `n = 0.5*dist` (origin at depth≈0.17), `f = 3*dist` covers back geometry.
3. **x/y half-width too small → shadow only in small center patch**: orthographic half-width used passed `half`(=12) only 24×24, most scene geometry outside `uv` treated as lit. Half-width `dist`(=24) covers 48×48.

**Lessons**: orthographic shadow projection near/far around "light-to-origin distance", near must leave margin; z mapping standard 0..1 orthographic not hand-written scale. After change confirm with `spirv-val` + actual run (stripes=precision / bias, disappearance=near clip, small patch=insufficient half-width).

## 17. Shadow camera eye position sign: eye = +direction_to_light (not -)

**Problem**: placed light eye at `-l*dist` (backlit side) looking at origin, renders depth map from **dark side**, shadows cast to wrong lit side — appears "all misaligned".

**Locating evidence**: `scene_bindless.slang` line 455 `n_dot_l = max(dot(n, light_dir), 0.0)` proves `light_dir` is "direction toward light", so light is on `+l` side. Shadow camera should be at `+l*dist` looking at origin:
```rust
let eye = [l[0]*dist, l[1]*dist, l[2]*dist];  // NOT -l*dist
```
**Lessons**: don't judge light direction by "looks like it should face there", check shader's actual `light_dir` usage (lit dot or light propagation). Reversed direction is "all misaligned" not "disappeared", easy to confuse with z mapping — investigate independently.

## 18. Vulkan shadow map sampling **must not** flip Y

**Problem**: `sample_shadow` had `uv.y = 1.0 - (proj.y*0.5+0.5)`, causing shadow vertically mirrored offset ("has shadow but misaligned").

**Root cause**: shadow map uses **orthographic projection without y-flip** (`proj[1][1]=+inv`, unlike main camera perspective `p[1][1]=-inv_tan` Vulkan y-flip). Vulkan framebuffer origin top-left, NDC y=-1 already maps to texture v=0, **during render `proj.y` directly corresponds `v=proj.y*0.5+0.5`, no flip needed**. Occluder (writes depth) and receiver (samples) use same `lightViewProj`, same `proj.y` computation, must look up **same texel**:
```hlsl
float2 uv = float2(proj.x * 0.5 + 0.5, proj.y * 0.5 + 0.5);  // removed 1.0 - flip
```
X direction never flipped and was correct, so misalignment only vertical — matches symptom.

**Lessons**: shadow map Y flip can't copy "GL habit". To decide whether to flip: see if shadow-map-generating projection **has** y-flip. Yes (main-camera-like) → flip on sample side; no (orthographic light projection) → don't flip.

## 19. Debug shadow priority order (avoid going in circles)

Investigate in this order, one variable per step:
1. **First confirm shadows enabled**: default `debug_flags` must contain `PBR_FLAG_SHADOW`(bit8) and `PBR_FLAG_DIRECT`(bit0), else no "direct light" to occlude, shadows show no change (this time default `0` = pure baseColor, true cause of "enabled but no response").
2. **No shadows at all vs all misaligned vs vertical offset vs stripes** each correspond to different root causes:
   - None / only small center patch → near clipping / insufficient half-width (§16)
   - All reversed misaligned → eye sign (§17)
   - Vertical mirror offset → sampling Y flip (§18)
   - Regular stripes → z mapping saturation + depth bias too small (§16 + §20)
3. **Depth bias**: under D32_SFLOAT `depth_bias_constant_factor` multiplies format min representable diff (≈2^-23), `1.0` effectively ≈0, large ground planes inevitably show acne stripes. Use `constant_factor≈64, slope_factor≈8` (fine-tune direction by acne / peter-panning).

**Cross-cutting experience**: shadow chain (direction derivation → projection z → near/far → half-width → eye sign → sampling Y flip → depth bias) each link independent, change one verify one symptom, don't move multiple variables at once.


## 20. MRT changes must sync all pipelines "drawing in same render pass" blend states

**Problem**: ForwardPass changed from 1 color attachment to 2 (HDR color + view-space normal MRT), validation reports:
```
vkCreateGraphicsPipelines(): pCreateInfos[0].pColorBlendState->pAttachments[1] is different
than pAttachments[0] and independentBlend feature was not enabled.
VUID-VkPipelineColorBlendStateCreateInfo-pAttachments-00605
```

**Cause**: SkyboxPass and Gizmo both draw in ForwardPass render pass (`ForwardPass::execute` draws skybox first, gizmo last). Each creates pipeline declaring only 1 blend attachment (`color_attachment_count: None` -> default 1), but render pass subpass now has 2 color attachments. Vulkan requires pipeline `VkPipelineColorBlendStateCreateInfo::attachmentCount` == subpass `colorAttachmentCount`. With 1 attachment happened to match; MRT broke it.

**Lessons**:
- **Render-pass attachment count is common divisor of all pipelines "drawing in it"**. Changing ForwardPass attachment count must sync SkyboxPass / Gizmo blend state count, even if they only write attachment 0 (`colorWriteMask = 0` for attachment 1 = "reserve but don't write").
- If don't want every pipeline writing full blend state, enable `independentBlend` feature (`PhysicalDeviceFeatures::independent_blend`, common on desktop + modern Android) allowing different blend config per attachment. Project chose "enable feature + each pipeline writes full" double insurance.
- `GraphicsPipeline::new` (`pipeline.rs`) originally only supported 0/1 attachment (`slice::from_ref`), extended for MRT to accept `color_blend_attachments: Option<&[...]>` slice.

## 21. `vkCmdBeginRenderPass` `clearValueCount` must >= max CLEARed attachment index + 1

**Problem**: ForwardPass changed to 3 attachments (color + depth + normal), reports:
```
vkCmdBeginRenderPass(): pRenderPassBegin->clearValueCount is 2 but there must be at least
3 entries... VUID-VkRenderPassBeginInfo-clearValueCount-00902
```

**Cause**: `execute` `clear_values` array only had `[color, depth]` 2, but attachment 2 (normal) also `LOAD_OP_CLEAR`. Clear values indexed by **attachment number**, so even if middle attachment not CLEAR, as long as highest-index CLEAR attachment is N, array needs N+1 elements (middle non-CLEAR slots ignored).

**Lessons**: after changing render-pass attachment count, immediately sync `execute` `clear_values` array length, order strictly corresponding to attachment number. Clear values themselves don't matter (normal fully covered by fragment shader), but count must be right.

## 22. Sampling depth attachment must add `SAMPLED` usage to image

**Problem**: GTAO pass sampling ForwardPass D32_SFLOAT depth reports:
```
vkUpdateDescriptorSets(): pDescriptorWrites[0].pImageInfo[0].imageView was created with
VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT, but descriptorType is VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE.
VUID-VkWriteDescriptorSet-descriptorType-00337
```

**Cause**: `DepthImage::new` (`render_pass.rs`) created depth image with usage only `DEPTH_STENCIL_ATTACHMENT`. Depth only used for in-scene depth test (`STORE_OP_DONT_CARE`), never sampled. GTAO needs to read it, must add `SAMPLED` usage.

**Lessons**:
- Vulkan image usage flags fixed at **creation**, can't add later. Any image "both attachment and sampled" (depth, normal MRT, HDR color) needs `| SAMPLED` at creation.
- Same for GTAO sampling normal MRT, `NormalImage::new` usage directly `COLOR_ATTACHMENT | SAMPLED`; PostPass sampling HDR color, ForwardPass color image also `COLOR_ATTACHMENT | SAMPLED` (reusing `NormalImage` helper).
- Depth attachment changed to `STORE_OP_STORE` (was `DONT_CARE`) to preserve content for post-pass sampling.

## 23. Per-frame-rewritten descriptor sets must be per-frame-in-flight, not global

**Problem**: ForwardPass AO descriptor set (set 4, each frame points to previous GTAO output) and PostPass HDR-input descriptor set both report:
```
vkUpdateDescriptorSets(): dstSet is in use by VkCommandBuffer... in the pending state.
VUID-vkUpdateDescriptorSets-None-03047
```

**Cause**: initially both passes allocated only **1** descriptor set, each frame `set_ao`/`set_input` rewrote it to point to new view. But frame N still running on GPU (pending), frame N+1 updated same set — validation judges "modified set in use by in-flight command buffer".

**Lessons**:
- **Any descriptor set rewritten each frame via `vkUpdateDescriptorSets` must allocate N copies by frame-in-flight** (N=2 here). Frame N only updates `sets[N]`, fence wait guarantees set no longer used by GPU.
- Contrast: shadow map descriptor set (ForwardPass set 3) never rewritten (view fixed once in `set_resources`), so 1 copy enough. Criterion: "will this set's binding be changed by `update_descriptor_sets` at runtime".
- GTAO depth/normal input descriptors also rewritten each frame, allocated `[frame][set]` 2D (4 copies: 2 frames × 2 sets, shader declares set 0 depth + set 1 normal).

## 24. Newly created image referenced by descriptor must first transition to declared layout

**Problem**: GTAO AO image created in `new`, first frame scene shader AO descriptor points to it (`ao_view((frame+1)%2)`), but AO image still `UNDEFINED` layout while descriptor declares `SHADER_READ_ONLY_OPTIMAL`. Reports:
```
vkQueueSubmit(): command buffer expects VkImage ... to be in layout SHADER_READ_ONLY_OPTIMAL
-- instead, current layout is UNDEFINED. VUID-vkCmdDraw-None-09600
```

**Cause**: first frame GTAO hasn't run (scene → gtao → post order, gtao after scene), but scene already sampling "previous frame" AO (`ao[(frame+1)%2]`, frame 0 = `ao[1]`, never written). Descriptor layout declaration and actual image layout mismatch — validation reports even if binding not truly sampled (`PBR_FLAG_AO` default off).

**Lessons**:
- **Cross-frame-dependent images (previous frame writes, this frame reads) transition to "read" layout immediately after creation**, making descriptor declaration valid. GTAO uses one-shot command buffer in `new` and `recreate_target` to transition both AO images `UNDEFINED` → `SHADER_READ_ONLY_OPTIMAL`.
- When pre-transitioned image later written by GTAO render pass, render pass `initial_layout = UNDEFINED` tolerates any incoming layout (with `LOAD_OP_CLEAR`), so pre-transition won't conflict with next write.
- Contrast: PostPass-read HDR color image needs no pre-transition — always ForwardPass writes first (`COLOR_ATTACHMENT_OPTIMAL`) then PostPass reads, even frame 1 scene runs first. Only cross-frame images where "read" happens before "write" need pre-transition.

## 25. Slang has no `gl_FragCoord`, use `SV_Position` input

**Problem**: GTAO shader wrote `float2(gl_FragCoord.xy) / pc.viewport`, slangc reports `error 30015: undefined identifier 'gl_FragCoord'`.

**Cause**: Slang/HLSL uses `SV_Position` semantic for pixel coords, doesn't expose GLSL `gl_FragCoord` builtin. Fragment shader `clipPos : SV_Position` input is pixel coords (`xy` = framebuffer pixel coords, top-left origin).

**Lessons**: always use `SV_Position` input for pixel coords in Slang (`float4 clipPos : SV_Position`, `clipPos.xy` = pixel coords), not `gl_FragCoord`. Fullscreen-triangle pass vertex stage already outputs `SV_Position`, fragment stage just takes it.

## 26. Push constant size needn't be multiple of 16

**Problem**: GTAO push constant struct computed 84 bytes (`inv_proj`(64) + `viewport`(8) + `radius`(4) + `mode`(4) + `_pad0`(4)), original comment "round up to 96 for 16-byte alignment", test assert `size_of == 96` failed (actual 84).

**Cause**: misunderstood Vulkan push constant alignment. Vulkan requires push constant range `size` only >= bytes shader actually reads, **not multiple of 16** (nor 4, though practice aligns to 4). Rust `#[repr(C)]` adds no tail padding, so 84 is 84.

**Lessons**:
- Push constant Rust mirror and Slang struct must match **byte-for-byte**, including tail padding. slangc struct also compact layout (no tail align padding), consistent with `#[repr(C)]`.
- Test assert actual computed bytes (84), not "aligned" 96. Vulkan guarantees at least 128 bytes push constants, 84 well under limit.
- Contrast UBO (std140) **must** 16-byte align (`FrameUBOData` 256 bytes, tail `_pad: [u32; 3]` for padding). Different rules, don't mix.

## 27. Cross-pass reading scene output: 1-frame latency pattern of previous-frame AO × this-frame IBL

**Background**: GTAO must run after ForwardPass (reads scene depth+normal), but IBL computed inside ForwardPass. Recomputing IBL in scene after GTAO would deadlock or require multiple passes.

**Solution**: GTAO each frame writes AO to `ao[frame]`, scene reads `ao[(frame+1)%2]` (previous GTAO output). 1-frame latency, common game practice.

**Lessons**:
- **Cross-pass data dependency with timing contradiction: 1-frame latency is standard solution**. Double-buffer AO images (by frame-in-flight), scene reads old frame, GTAO writes new frame, no interference.
- Fast camera movement makes AO "trail" one frame — acceptable; next step can add temporal filter to smooth.
- Half-res GTAO + scene linear-sampler upsample to full res, GTAO low-frequency signal, slight blur acceptable. Follow `DESIGN.md` §2.2 performance (heavy pass half-res, RayQuery already `scale=0.5` precedent).

## 28. ForwardPass render to intermediate HDR target + split PostPass: bigger change than expected

**User request**: "split out postpass, put reinhard/aces in postpass". Seems just moving tonemap from `scene_frag.slang` to new pass, actually touches everything:

- ForwardPass no longer renders directly to swapchain, instead to `R16G16B16A16_SFLOAT` HDR intermediate target (one per swapchain image, like depth image).
- `set_target` signature changes (no longer takes `swapchain_views`, takes `image_count`), framebuffer attachments from `[swapchain_view, depth]` to `[hdr_view, depth, normal_view]`.
- `color_format` field changes from swapchain format to HDR format.
- PostPass new: own render pass (swapchain format, `initial_layout = UNDEFINED` tolerates previous PRESENT_SRC_KHR), per-swapchain-image framebuffer, per-frame-in-flight descriptor set (reads HDR view), tonemap push constant.
- `GraphRenderer::render` frame flow from `scene -> egui/barrier` to `scene -> gtao -> post -> egui/barrier`, swapchain `COLOR_ATTACHMENT_OPTIMAL -> PRESENT_SRC_KHR` transition point moves from after ForwardPass to after PostPass.

**Lessons**:
- **"Splitting X into new pass" usually entails "original pass output target changes"**, because new pass reads original output as input texture, original can no longer write directly to final target (swapchain). Evaluate "should original pass render target become intermediate" before starting.
- Intermediate target lifecycle synced with swapchain (`drop_target` / `recreate`), no leak. One intermediate target per swapchain image (not per frame-in-flight), because framebuffer indexed by image index.
- After tonemap split, scene shader outputs linear HDR; swapchain is sRGB, PostPass sRGB encoding automatic when writing swapchain (Vulkan `B8G8R8A8_SRGB`). Don't manually `pow(1/2.2)` in PostPass, that double-encodes.

## 29. Validation error priority: fix "creation-time" errors first, then "runtime" errors

GTAO+PostPass integration exposed 5 validation errors at once, fix order:

1. **`independentBlend` feature not enabled** (pipeline creation) - enable feature in `context.rs`
2. **`clearValueCount` insufficient** (render pass begin) - fix `execute` clear_values array
3. **depth image missing `SAMPLED` usage** (descriptor write) - fix `DepthImage::new` usage flags
4. **descriptor set occupied by in-flight command buffer** (descriptor write) - change to per-frame-in-flight allocation
5. **AO image layout UNDEFINED mismatches descriptor declaration** (queue submit) - pre-transition after creation

**Lessons**:
- Validation errors reported in Vulkan call order (create -> record -> submit), **earlier errors may mask later ones**. Fix one run once, don't guess all at once.
- Error 4 (in-flight descriptor) initially masked by noise from error 3 (usage mismatch) — after fixing 3, saw 4 as independent. Investigate each VUID separately, don't assume "all caused by descriptors".
- `RUST_LOG=info,prism_render=trace` + logging to file (`target/debug/template.log`, main.rs logger auto-writes file when non-terminal) is standard for validating render pipeline: grep `validation|ERROR|VUID` count, grep `ForwardPass:|GtaoPass:|PostPass:` confirm each pass executing.

## 30. CI drift / toolchain class lessons (pitfalls getting CI green this time)

**Background**: one `git pull` brought many new shader sources (added `gtao.slang` / `post.slang` / `skybox.slang`, renamed `mesh->mesh_vert`, `bindless->scene_frag`, `shadow->shadow_depth`), but nobody ran `shaders/compile.sh` + `xtask shader-bindgen` to regenerate artifacts, all 5 CI jobs red. 4 commits to fix.

### 30.1 lint class (fmt + clippy)

- **`cargo fmt --all -- --check` must pass**: pulled code not matching current rustfmt version makes `cargo fmt --all` change many files (even not your logical changes), CI first step fails. Run `cargo fmt --all` locally before commit.
- **clippy uses CI rustc version, not local**: CI uses latest stable, local Termux rustc may be one or two minor versions older, missing new lints. This time `crash_dialog_linux.rs` / `macos.rs` `std::io::Error::new(ErrorKind::Other, ...)` triggered `io_other_error` lint (only CI new rustc), local 1.96 didn't report. **Lints not reproducible locally only caught via CI iteration**. Root fix: `ErrorKind::Other` → `std::io::Error::other(...)`.
- Under clippy `-D warnings` all `too_many_arguments` (threshold 7) must be refactored, no `#[allow]`**: project convention fix root cause. Split 8+ params into parameter-group structs (`barrier2` `ImageBarrier` + `color_subresource`; `transition_image_single` takes `vk::ImageSubresourceRange`; `RenderGraph::execute` takes aggregate `&RenderContext`).
- Other common clippy root fixes (all landed): `and_then(|x| Some(y))` → `map`; derive `Default` with `#[default]` if possible; drop `x as usize` when `x: usize`; `bits = (bits<<16)|(bits>>16)` → `bits.rotate_right(16)`; truncate overly precise float literals; drop `clone()` on Copy types; `to_json(&self)` → `to_json(self)` for Copy; `write!(..,"{}\n")` → `writeln!`; merge combinable `if let`; omit explicit useless lifetimes.

### 30.2 shaders job environment dependencies

- **`shaders/compile.sh` depends on `spirv-tools` (`spirv-val`)**: CI shaders job installs via `sudo apt-get install -y spirv-tools`, as SPIR-V validity gate. Old `fix_spirv.py` (stripping illegal `ArrayStride` of bindless runtime arrays, §13) removed with Slang 2026.21, `spirv-dis` / `spirv-as` no longer required.
- **`shaders/compile.sh` keeps only unified gate at end**: every `.spv` passes `spirv-val`, no per-shader post-processing.

### 30.3 Drift check: changed shader source must regenerate and commit artifacts

- CI shaders job has **drift guard** at end: run `shaders/compile.sh` recompile `.spv` + reflection JSON, then `xtask shader-bindgen` regenerate `crates/prism-render/src/shader_bindings.rs`, then `git diff --quiet` compares committed `.spv` and `shader_bindings.rs`. **Any mismatch → `exit 1`**.
- **Rule: touching `shaders/slang/*.slang` requires local `bash shaders/compile.sh` + `cd xtask && cargo run --bin shader-bindgen -- ../shaders/reflection ../crates/prism-render/src/shader_bindings.rs`, commit generated `.spv` / `reflection/*.json` / `shader_bindings.rs` together**. Otherwise CI red.
- **Can't run `shaders/compile.sh` on Termux**: slangc official prebuilt is glibc binary, Termux (bionic) can't run; and `.spv` `include_bytes!` into binary, must use **same slangc version as CI** (currently `2026.21`) to match. `compile.sh` now rejects older `slangc`, fails when reflection JSON mismatches repo (see #980), so version mismatch exposed at compile step not bindgen drift. This time since no local slangc, borrowed **CI artifact** (drift step pre-`Upload compiled SPIR-V + reflection (debug)` uploaded recompiled `.spv` and reflection JSON): downloaded artifact `.spv` + `reflection/*.json`, locally only ran `xtask shader-bindgen` (only eats reflection JSON, no slangc needed) to generate `shader_bindings.rs`, overwrote and committed.
- **`.spv` and `shader_bindings.rs` must update together**: updating one still fails drift (drift checks both).

### 30.4 Environment / operations

- **`rm -rf` forbidden by system on Termux**: clean directories with `python3 -c "import shutil; shutil.rmtree(path)"` or `rmdir` (empty dir), not `rm -rf`. Single-file `rm` without `-r` fine.
- **crates.io direct TLS flaky**: Termux `static.crates.io` downloads occasionally `SSL connect error`. Configure `~/.cargo/config.toml` to use domestic mirror (currently `rsproxy`: `registry = "sparse+https://rsproxy.cn/index/"`) stable.
- **CI failure first grab `gh run view <id> --log-failed`**: GitHub API occasional 503, use retry loop; real error at end of `--log-failed` (above `##[error]Process completed with exit code 1`). This time chain: `lint(clippy)` → `shaders(spirv-tools missing)` → `shaders(drift: stale shader artifacts)` three layers, iterate layer by layer.

## 31. Scene load time breakdown + redundant copy elimination (Sponza 7.6s -> 1.8s)

**Background**: Sponza load log only one line `gltf parse+import: 5430ms` and `texture upload: 1498ms`, can't see where time went. Added stage timing to locate, then fixed biggest redundant copy.

### 31.1 First break down logs, then optimize

**Approach**: insert `Instant::now()` by stage in `gltf_loader::load`, split `parse+import` into parse JSON / import buffers / decode images / materials / textures / meshes / scene nodes seven segments; in `load_demo_scene` split `texture upload` / `mesh upload` each into collect (clone scene_store data) + upload (register_*_into), count pixel / vertex totals.

**Truth revealed** (Intel Sponza 2022 test asset: 72 4K textures = 1208 MP, PNG disk 1.89 GB → RGBA8 4.50 GB; 405 meshes / 2M verts). Note this asset **doesn't represent real game load**, see §31.7.

| Stage | Time | Share |
|------|------|------|
| `gltf phase: decode images` | 713ms | 14% |
| **`gltf phase: textures`** | **4406ms** | **84%** ← biggest |
| `gltf phase: meshes` | 86ms | 2% |
| `texture upload collect` (clone) | 543ms | - |
| `texture upload` (per-tex pixels.clone) | 1001ms | - |
| `batch upload submit+wait` (GPU) | 540ms | - |

**Lessons**: aggregate logs ("X total Nms") useless for locating bottleneck. Any stage over ~200ms needs another split, with scale metrics (total texture megapixels / vertex count / index count), to distinguish "many objects" vs "single large resource". This time `gltf phase: textures (72 images, 1208.0 MP)` one line immediately exposed 4.5 GB pixel data repeatedly copied. **Another pitfall**: can't estimate memory-copy cost by PNG disk size (1.89 GB) — PNG decompresses to RGBA8 expanding ~2.4×; real memory pressure is decompressed pixel bytes (1208M × 4 = 4.5 GB).

### 31.2 `to_rgba8` doing meaningless full copy (root cause of 4406ms)

**Problem**: `import_images_parallel` already converted each image to `R8G8B8A8` (`dyn_image.to_rgba8()`), but `to_rgba8(&image)` did three more passes over this 4.5 GB: `image.pixels.clone()` (4.5 GB read + 4.5 GB write) + `Vec::with_capacity(count*4)` (4.5 GB alloc) + per-4-byte `expand_to_rgba` copy (4.5 GB write, non-memcpy). ≈18 GB memory throughput, at ~3.3 GB/s ≈5.4s, matching observed 4406ms.

**Fix** (`crates/prism-asset/src/gltf_loader.rs`):
- `to_rgba8` takes `GltfImageData` by value (`&GltfImageData` -> `GltfImageData`), call sites `images.iter()` -> `images.into_iter()`.
- RGBA8 source format fast path: `return Ok(RgbaPixels { pixels: image.pixels })`, zero-copy move.
- Non-RGBA8 source slow path, **`log::warn!`** reports resolution + format + channel count — our parallel import always produces RGBA8, hitting slow path means another code path feeds non-RGBA8, must be visible.
- Effect: `gltf phase: textures` 4406ms -> **0ms**.

**Lessons**: conversion function when "input already target format" must take zero-copy move, not unconditional clone + per-element processing. Especially GB-scale pixel / vertex data, one redundant copy is seconds. `image.pixels.clone()` + `Vec::with_capacity` + `chunks_exact` trio is typical "safe but wasteful" pattern, once scale metric (1208 MP) known can locate immediately.

### 31.2 Two redundant clones in upload path (collect + per-tex pixels.clone)

**Problem**: `load_demo_scene` texture upload loop did two 4.5 GB clones:
1. `texture_data: Vec<_> = scene_store.textures().map(|(h, d)| (h, d.clone())).collect()` -- 543ms
2. In loop `pixels: data.pixels.clone()` into `TextureUploadInput` — again

**Fix** (`crates/prism-engine/src/app.rs` + `crates/prism-asset/src/scene_store.rs`):
- Add `textures_mut() -> impl Iterator<Item = (TextureHandle, &mut TextureData)>` to `SceneStore`.
- Upload loop changed to drain mode: `std::mem::take(&mut data.pixels)` moves pixel buffer directly into `TextureUploadInput`, store keeps empty `Vec`. After upload CPU-side pixels are dead weight (GPU has device-local copy), drain safe.
- Borrow pitfall: `for (h, data) in self.scene_store.textures_mut()` holds `&mut scene_store.textures`, can't call `self.scene_store.take_texture_pixels(h)` in loop (second mutable borrow). Right approach: `mem::take(&mut data.pixels)` directly on `&mut TextureData` from iterator, no extra accessor — so final didn't keep `take_texture_pixels` method, only `textures_mut`.
- First collect `(asset_h, TextureUploadInput)` into local Vec, letting `&mut self.scene_store` borrow end before calling `&mut self.renderer`, avoiding self borrow conflict.
- Effect: `texture upload collect` 543ms -> **0ms**, `texture upload` total 1545ms -> **330ms**.

**Lessons**:
- **CPU-side data not read after upload should be drained, not cloned**. Criterion: grep all call sites of store accessor, if after upload only metadata queries like `.count()` remain (this project `scene_store.textures()` only `.count()` in summary log), draining pixels is safe.
- On borrow conflict prefer "operate directly on existing `&mut` reference", don't add symmetric `take_xxx(handle)` accessor — latter triggers "second mutable borrow of same struct". `textures_mut` + `mem::take` is standard pattern for draining slotmap table in Rust.
- `TextureUploadInput` owning `pixels: Vec<u8>` is reasonable (`reserve_into` only borrows once for staging map), so moving in needs no manager signature change.

### 31.4 Mesh upload path untouched (deliberately)

`mesh upload: collect=23ms upload=147ms` already small (2M verts ≈ 100 MB, clone cost negligible), and mesh data may still be read by scene_store after upload (unlike textures clearly safe to drain). **Deliberately not optimized**: first principle of optimization is look at scale metrics, don't cut 23ms stage.

### 31.5 Bottleneck distribution after fix (~1.8s, acceptable)

```
gltf parse+import:        805ms  (45%)
  └ decode images:        701ms  (39%)  ← PNG/JPEG decode, rayon parallel
  └ import buffers:        23ms
  └ meshes:                74ms
batch upload submit+wait: 522ms  (29%)  ← actual GPU PCIe transfer
texture upload (CPU):     330ms  (18%)  ← 72 vkCreateImage + alloc + staging map
mesh upload (CPU):        168ms   (9%)
```

Remaining big items are real work, not redundant copies.

### 31.6 Next-step optimization directions (not done, sorted by cost-effectiveness)

0. **GPU block-compressed upload (highest cost-effectiveness, root memory fix)**: currently all textures uploaded as `R8G8B8A8_UNORM`, 4.5 GB device-local + 4.5 GB host staging root cause of asset memory explosion (§15 records as PBR color-correctness defect). Switch to BC7 (0.5 bytes/pixel): GPU ~1.1 GB, staging ~1.1 GB, CPU peak ~2.3 GB. Needs: (a) glTF load stage BC7-encode RGBA8 (CPU, `bc7enc` crate); (b) `TextureUploadInput` / `BatchUploader::upload_image` support BC7 format + no mip blit (BC-compressed blit needs re-encode). **Higher priority than decode images below**, solves memory (GB-scale) not just load time (seconds). Note albedo/emissive should upload sRGB variant (`BC7_SRGB`), normal/MR keep UNORM (fix §15 color-space defect together).

1. **decode images 701ms (next cost-effectiveness)**: 72 4K PNG decoded by `image` crate, already rayon parallel. Further compression only by changing decoder:
   - `zune-image` / `zune-png` (Rust native, SIMD, 2-3× faster than `image` PNG)
   - or `lodepng` pure PNG path
   - Note: changing decoder must keep "output RGBA8" contract, else `to_rgba8` fast path breaks (hits slow path and warns).
2. **texture upload 330ms (medium)**: 72× `vkCreateImage` + `vkAllocateMemory` + staging map. Switch to suballocator (`gpu-allocator` crate or custom bump allocator) batching per-resource allocations, estimated halve. Larger change, touches `BatchUploader` and `RenderTextureManager` memory ownership.
3. **batch upload submit+wait 522ms (low)**: 4.5 GB pixels + 100 MB mesh over PCIe, near physical bandwidth limit. Can't compress unless async / timeline semaphore overlaps upload with first-frame render (follow-up mentioned in `BatchUploader` comments). Async is architectural change.
4. **Lazy / streaming upload (architectural, most thorough)**: change "load all then render" to "load low-res mipmap 0 first visible immediately, background stream high mips". Requires changing `SceneStore` ownership model and pipeline "texture ready" determination, large work, milestone-level refactor.

**Current ~1.8s is already reasonable for this 4.5 GB RGBA8 extreme asset (real game scenes much smaller, see §31.7)**. Core lesson of this round is 31.1: **break down logs to locate, then find redundant copies by scale metrics**, don't guess bottleneck by feel. `to_rgba8` zero-copy fast path + drain upload are two universal fixes for "GB-scale data movement" anti-patterns, directly reusable when adding new resource types (audio, animation).

### 31.7 Test asset doesn't represent real game load (important attribution)

All numbers from this optimization (4.5 GB pixels, 9 GB CPU peak, ~1.8s load) are based on **Intel Sponza 2022** test asset, whose texture scale **far exceeds real games**, can't infer engine performance in production scenes from its absolute numbers.

**Sponza version comparison**:

| Asset version | Texture count | Per-texture size | Total pixels | RGBA8 decompressed | Purpose |
|----------|--------|----------|----------|-----------|------|
| Crytek Sponza (classic, common in games) | ~20 | mixed 512/1024/2048 | ~200 MP | ~0.8 GB | Real-time render benchmark |
| **Intel Sponza 2022 (used here)** | **135** | **all 4096×4096** | **2184 MP** | **~8.5 GB** | Offline render / reference images |
| Typical AAA game Sponza-class scene | ~30-50 | mixed 1K/2K/4K | ~400 MP | ~1.5 GB | ~0.4 GB with BC compression |

glTF-referenced subset is 72 full 4K (1.89 GB PNG -> 4.5 GB RGBA8), already extreme. **No real game gives Sponza full-4K textures** — Intel Sponza 2022 is a 4K scan asset for path-traced reference images, not designed for real-time rendering.

**How real games control texture memory** (next engine extension should support, by priority):

1. **GPU block compression (BC1/BC3/BC7/BCN)** — §31.6 item 0. 4096×4096 RGBA8 = 64 MB, BC7 = 16 MB (4:1), BC1 = 8 MB (8:1, no alpha). Root means for real games keeping texture memory under GB. Current all-RGBA8 upload is known defect (§15), not just color but memory.
2. **Mip-chain streaming**: only load mips needed by visible distance. Full 4K mip chain = 21 MB, but only 1-2 nearest objects need 4K, far use 1K/512. Real games dynamically select mip by "surface distance to player", CPU/GPU memory peak far below full load.
3. **Virtual texturing (VT)**: AAA games slice GB-scale textures into 64KB pages loaded on demand, only visible pages resident. Only way to make "full 4K Sponza" runnable, but complex (page table, page fault, feedback channel), engine-level milestone.
4. **Resolution tiering**: not all surfaces get 4K. Real artists mix 1K/2K/4K by "how close / how often seen", not indiscriminate full 4K like Sponza 2022.

**Actual impact on this project**:

- Engine itself **has no memory leak or waste** (§31.2/§31.3 redundant copies fixed). 4.5 GB is true decompressed size of this asset, any engine faces same pixel volume.
- But engine **lacks texture pipeline essential for real games**: block compression, streaming, resolution tiering all missing. Next extension milestone work, not a bug.
- **Test suggestion**: next performance baseline should use Crytek Sponza or custom "mixed-resolution" scene, not Intel Sponza 2022, else optimization decisions skewed by extreme asset (e.g. building suballocator for 4.5 GB when real scene 0.4 GB doesn't need it).

**Lesson**: performance conclusions must be bound to test asset. Bottleneck distribution from extreme asset (Intel Sponza 2022 full 4K) can't be extrapolated to production. When recording timing numbers, note asset scale alongside ("72 4K textures = 4.5 GB RGBA8"), else future readers think "engine loads Sponza in 1.8s" is universal, while Crytek Sponza enters in a few hundred ms.

### 32 IBL CPU convolution disk cache

**Background**: `IblResources::new()` performs three CPU convolutions + BRDF LUT integration on HDR env map at engine startup, ~4127ms total (Intel Sponza 2022 + 1K HDR), longest single freeze in startup (~64%). BRDF LUT (3260ms) = 79% of IBL total.

#### 32.1 Bottleneck distribution (before cache)

| Convolution step | Time | Share | Notes |
|----------|------|------|------|
| BRDF LUT (512x512, 1024 samples) | 3260ms | 79% | double loop 512x512x1024 = 268M iterations, densest |
| Prefiltered convolution (5 mips, 128 samples) | 792ms | 19% | second hotspot, mip chain multi-pass |
| Irradiance convolution (64x64, 4096 samples) | 75ms | 2% | low res, short |
| Total | 4127ms | 100% | |

#### 32.2 Disk cache design

**Core idea**: cache CPU convolution results to disk, next startup skip compute and directly load f32 raw data back into `Vec<f32>`, Vulkan upload pipeline (f16 conversion + staging buffer + submit) unchanged. (Cache root later unified to `<exe dir>/cache/ibl/`, see "cache root" paragraph at end.)

**Cache key scheme**:

- **BRDF LUT**: fixed filename `brdf_lut_512.bin`, no dependencies (pure GGX Smith math, same for any scene / HDR). Cache always valid.
- **Env-related (cubemap / irradiance / prefiltered)**: `std::hash::DefaultHasher` (SipHash-1-3) on `env_bytes` (raw .hdr bytes), hex-encoded as dir name. HDR content change → different hash → new cache automatically.

**File format**: `u32 LE (element count) + f32 LE × count`, with length validation. 9 files total:

| File | Size | Notes |
|------|------|------|
| `<cache>/ibl/brdf_lut_512.bin` | ~2 MB | 512×512×2 floats, HDR-independent |
| `<cache>/ibl/<hash>/cube_512.bin` | ~24 MB | 6×512×512×4 floats |
| `<cache>/ibl/<hash>/irradiance_64.bin` | ~393 KB | 6×64×64×4 floats |
| `<cache>/ibl/<hash>/prefiltered_mip{0..4}.bin` | ~2.1 MB | Each mip separate, easy single-mip hit |
| **Total** | **~28.5 MB** | ~28MB disk per HDR env |

**Later evolution (current form)**: CPU convolution + disk cache path **fully replaced**, `cache_paths` module and `GraphRenderer::set_ibl_cache` injection deleted.

IBL now from **offline GPU bake**: `assets/shaders/slang/ibl_bake.slang` (`ibl_brdf_lut` / `ibl_equirect_to_cube` / `ibl_irradiance` / `ibl_prefilter` four compute entry points) baked into `prism_gfx::ibl::CookedIbl` in asset pipeline, runtime `crates/prism-render/src/ibl.rs` only `decode` + staging upload, no runtime convolution, no disk cache dir.

**Lesson still holds**: runtime-generated artifacts can't go in `assets/` (read-only build input, unwritable after `Program Files` install, even more inside APK on Android). Just solution changed from "runtime compute + cache" to "build-time compute + direct asset read", cache problem disappears.

#### 32.3 Tradeoffs

| Tradeoff | Choice | Reason |
|------|------|------|
| Storage format | f32 raw array (not f16) | directly usable on write-back, no conversion; 28MB acceptable |
| Hash algorithm | FNV-1a not `DefaultHasher` | `DefaultHasher` algorithm not guaranteed stable across Rust versions, toolchain upgrade silently invalidates all caches leaving orphan dirs; FNV-1a same source as `prism-reflect` `TypeId` |
| Cache granularity | per-file (9 files) not single blob | incremental invalidation: HDR change only rebuilds env-related, keeps BRDF LUT |
| Load timing | synchronous blocking (startup) | IBL must be ready for first frame; async next extension PR |
| Error handling | any I/O failure falls back to recompute | idempotent: computed result matches cache |

**Why not more complex solutions**:

- **LZ4 compression**: 28MB not worth it, f32 data near white noise (random sampling), terrible ratio.
- **Async / background load**: first frame needs IBL (scene reflections), async needs no-IBL-then-pop-in transition logic, complexity far exceeds gain.
- **GPU convolution**: IBL convolution (irradiance / prefilter) faster on GPU compute (~5-10ms), but BRDF LUT is central bottleneck and GPU convolution needs extra compute pipeline + sync logic.

#### 32.4 Results

| Metric | Before cache | After cache | Speedup |
|------|--------|--------|--------|
| IBL convolution total | 4127ms | **1ms** | **~4000×** |
| Renderer init (incl. IBL) | 4441ms | 233ms | 19× |
| Startup to first frame (incl. scene load) | 6419ms | **2075ms** | **3.1×** |

#### 32.5 Lessons

1. **BRDF LUT easiest optimization**: no external input, pure math, cache always valid, 3260ms→0ms with dozens of lines. Any engine's first IBL optimization should be BRDF LUT cache.

2. **Per-stage breakdown locates bottleneck**: from vague "IBL: 3678ms" into 8 sub-stage logs (HDR decode / generate_cubemap / convolve_irradiance / prefiltered / brdf_lut / create_images / upload / views), instantly locked BRDF LUT as top bottleneck, avoiding futile optimization of irradiance or prefilter.

3. **Content hash better cache key than timestamp**: file mtime reset on git checkout / copy, causing unnecessary invalidation. Content hash (SipHash of HDR bytes) guarantees "same content same cache".

4. **Cache path design for incremental invalidation**: BRDF LUT and env-related data separate dirs, HDR change doesn't break BRDF LUT cache.

5. **f32 raw array simplest serialization**: no schema, no dependencies, zero-copy deserialization (`ptr::copy_nonoverlapping`). For known-size arrays, `u32 length prefix + f32 data` enough.

6. **"Cache invalidation" isn't the problem, dependency is**: BRDF LUT zero dependency → always valid; env data only depends on .hdr content → content-hash-driven invalidation. No ambiguity of "I changed config but cache didn't refresh".

#### 32.6 Next extension directions (not in this scope)

- **Skybox componentization**: package HDR + precomputed IBL data + BRDF LUT into reusable skybox resource component (ECS or asset), support runtime env-map switching without rebuilding `IblResources`.
- **GPU convolution**: irradiance and prefilter on GPU compute (~5-10ms vs CPU ~900ms), directly generating Vulkan images instead of CPU f32 staging, skipping f16 conversion + upload.
- **BRDF LUT samples configurable**: currently fixed 1024, expose as constant or runtime param (512 ~800ms, 256 ~200ms, quality diff < 0.5 dB).
- **IBL load progress feedback**: show "loading environment..." at startup or background thread + semaphore, avoid unresponsive window.

## 33. GI baker two big bugs: inside-solid misses walls + slangc instruction scheduling corrupts SH coefficients

**Scenario**: scene multicolored after enabling GI. User suspected "maybe point lights counted". Actually baker uses only one directional light start to finish; multicolor is two independent bugs stacked.

### 33.1 inside-solid detection misses wall interiors

**Problem**: baker inside-solid detection (determine which probes inside wall / floor solids and zero their SH) used "all 6 axis-aligned short rays hit" criterion. Fails on real walls:

- **Thin wall**: probe inside wall panel, 2 rays along normal hit near surface, but 4 rays along wall plane escape through panel edge -> 2/6 hits -> missed, wrong bright / dark SH inside wall leaks into probe volume.
- **Wall edge near doorway**: some axis ray escapes through doorway -> 5/6 hits -> missed.

**Cause**: all-6-hit too strict. Any axis ray escaping (thin-panel edge, doorway, opening) causes miss, and real walls full of such cases. Controlled experiment (old vs new logic) Sponza inside-solid rose 23 → 112 (5× more wall-interior probes identified).

**Fix**: use "backface hit + short distance" two signals, folded into main ray loop (each hit already samples triangle normal for bounce lighting, only extra dot + compare):

- **Backface** (geometry normal same direction as ray = ray leaving solid) distinguishes "inside wall" vs "inside room": room inner surface is front hit, solid outer surface is back hit.
- **Short distance** (`CommittedRayT() < test_len` = half probe spacing) distinguishes "inside wall" vs "inside large hollow shell" (e.g. test cube): both see backfaces, but shell inner surface at room scale, far beyond test_len.

Both signals necessary: backface alone misjudges hollow shell, distance alone misjudges narrow corridor. Threshold `≥50%` rays short-distance backface hit, tolerating few rays escaping thin-panel edge / doorway still classified inside-solid. Test cube (hollow shell) verified not misjudged (inside-solid=1/216, only boundary probe), proving "short distance" signal distinguishes "inside wall" from "inside large hollow shell".

### 33.2 slangc instruction-scheduling bug corrupts SH coefficients (true root of multicolor)

**Problem**: 27.7% probe DC (c=0, average irradiance) negative, minimum -7.478. DC = `Σ radiance·C0·w`, every term non-negative, **must be ≥ 0 mathematically**. Negative SH reconstructed by `EvalSH9(normal, sh)` produces arbitrary large positive/negative values by normal direction, times albedo/π = full-screen color patches.

**Investigation (control-variable experiments locating faulty layer)**:

1. Force `radiance=(1,1,1)` -> negative DC zeroed -> readback / memory layout innocent, problem in radiance computation.
2. Force `hit_albedo=(1,1,1)` -> negative DC zeroed -> suspect locked to albedo read.
3. CPU-side vertex colors all ∈ [0,1] + indices all in-bounds -> data innocent, problem in GPU read.

**Root cause**: slangc 2025.11 does wrong instruction scheduling for `vertexData.Load3(... + COLOR_OFFSET)` color load, returning garbage (incl. negatives). Signature: position load normal, color load abnormal (same buffer, same offsets); R/G/B channels **independently** corrupted (not uniform sign error, reading adjacent memory).

**Resistance of this bug (all following ineffective)**:
- `-O0` (not optimization-level, scheduling-path problem)
- `Load4` instead of `Load3` (not 3-component-specific path)
- Function extraction (inlined)
- Consecutive loads, unused `CommittedRayT()`, `0.0 * CommittedRayT()` (DCE'd)
- Simple never-taken `continue` branch

**Only effective** fix: after color interpolation, re-consume `v0/v1/v2` (positions) with same barycentric weights, reconstruct hit position `recon = w0*v0 + bary.x*v1 + bary.y*v2`, compare with `world_pos + dir*CommittedRayT()`, use `step(err, 0.5)` multiplied onto albedo. Normally `recon ≈ hit_pos`, multiplier 1.0 (no side effect); this computation forces compiler to keep v0/v1/v2 alive across color load lifecycle, avoiding scheduling bug. Sentinel experiment proved check never fires (no 999 markers in max DC) — "semantic no-op" fixed bug.

### 33.3 Lessons

1. **Mathematical invariants strongest bug signal**. DC must ≥ 0, baked -7.478 directly violates axiom. Early logs had `DC stats: min [-7.478,...]` but dismissed as "dark interior" normal. **Abnormal numbers themselves scream bug**, don't mask math contradiction with "looks reasonable" visual impression.

2. **Control-variable experiments beat speculative reasoning**. "Force inject known-correct value" bisection (radiance=1 / albedo=1) halves suspicion each experiment, three rounds locked to "GPU color load". Much faster than reading code line by line.

3. **Build baseline first, judge bug new or old**. Once suspected introduced by own inside-solid change. `git stash` restore old shader ran once, DC data byte-identical -> confirmed pre-existing bug. **After change breaks, prove causality before fixing**, don't claim ownership just because "I just changed this".

4. **"Semantic no-op" can fix bug, but must leave evidence**. recon check zero effect normally (never fires) yet fixed bug, extremely counter-intuitive, easily deleted as dead code later. Comments must state: it is **both** real data-integrity check **and** workaround for slangc scheduling bug, both roles indispensable. **Workarounds need "why can't delete" comments**.

5. **Verify assumptions at minimum cost before diving**. During investigation assumed three wrong root causes (NaN from degenerate triangles / `-O` optimization bug / StructuredBuffer replacement), each consuming a full GPU bake (~30s). StructuredBuffer approach didn't account std140 padding float3 to 16 bytes, stride 56→80, worse. Calculate memory impact before changing layout.

  6. **slangc bug signature**: produces "mathematically impossible" values + channel-independent corruption + partial load anomaly in same buffer. Same class as §13 (Slang illegal `ArrayStride` on opaque runtime arrays), compiler-correctness issues `spirv-val` can't catch (SPIR-V legal but semantically wrong), only found via runtime data invariants (DC ≥ 0).

## 34. Device-lost investigation: fence sync + TDR + BLAS index double offset

**Background**: Sponza scene load (405 instances) triggers `VK_ERROR_DEVICE_LOST` during BLAS/TLAS build. Initially looked like `queue_wait_idle` sync issue, actually three nested layers.

### 34.1 Layer 1: `queue_wait_idle` + no fence isn't reliable sync

**Symptom**: `vkFreeCommandBuffers(): pCommandBuffers[0] is in use` validation error → device lost.

**Problem**: `submit_and_wait` helper used `vkQueueSubmit(queue, &submit, VK_NULL_HANDLE)` + `vkQueueWaitIdle(queue)` to wait GPU. `queue_wait_idle` may return before validation completes on Windows drivers, causing command buffer freed while GPU still executing. Validation caught freeing in-use command buffer, driver internal state corrupted, eventually device lost.

**Fix**: replace all with dedicated fence pattern:
```rust
// ❌ not reliable
device.queue_submit(queue, &submit, vk::Fence::null());
device.queue_wait_idle(queue);
device.free_command_buffers(pool, &cmds);

// ✅ reliable
let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
device.queue_submit(queue, &submit, fence)?;
device.wait_for_fences(&[fence], true, u64::MAX)?;
device.destroy_fence(fence, None);
device.free_command_buffers(pool, &cmds);
```

**Lessons**:
- `queue_wait_idle` isn't substitute for `wait_for_fences`. May return early on some drivers, `wait_for_fences` guarantees that specific submit completed.
- `buffer::upload_to_buffer` already had correct fence pattern (comments explain why not `queue_wait_idle`), but `acceleration_structure.rs`, `ibl.rs`, `gtao.rs`, `scene_scope.rs` each had independent `submit_and_wait` copy, all wrong. **Vulkan helpers breed fragmented wrong copies**, should unify to one shared helper.
- Only appropriate use of `queue_wait_idle` is "drain" before program exit: ensure GPU idle before releasing all resources.

**Affected code locations** (6, all fixed):

| File | Function/location | Purpose |
|------|-----------|------|
| `acceleration_structure.rs` | `submit_and_wait` | BLAS + TLAS build |
| `ibl.rs` | `submit_and_wait` | IBL upload |
| `gtao.rs` | `transition_ao_images_to_shader_read` | GTAO image transition |
| `scene_scope.rs` | `upload_probe_volume` | GI probe volume upload |
| `prism_bake_gi.rs` | two inline | bake readback |
| `prism_bake_image.rs` | two inline | image readback |

### 34.2 Layer 2: 405 individual BLAS builds trigger Windows TDR

**Symptom**: after fence fix validation errors gone, but device lost still occurs, time changed from ~200ms to ~5s. New time exactly sum of submitting each BLAS build to GPU and waiting single fence.

**Problem**: 405 BLAS builds submitted separately (each with fence create/submit/wait/free/destroy), producing ~5s continuous execution. `batch upload submit+wait` ~750ms (textures same data volume) hints BLAS build shouldn't be 6× slower than texture upload.

**First fix — single batch submit**: record all 405 BLAS builds into one command buffer, submit once, wait once. Eliminated ~405× submit overhead, but continuous GPU execution still ~5s, triggering Windows TDR (default 2s timeout).

**Second fix — chunked submit**: split 405 BLAS builds into batches of 64 (7 batches), each with one command buffer + one submit + one fence wait. Each batch GPU time < 2s, no TDR, submit overhead only 7 not 405.

**Lessons**:
- Windows TDR (Timeout Detection and Recovery) resets GPU when continuously unresponsive over 2s. Large GPU work (hundreds of BLAS builds, big compute dispatches) must split into smaller batches with fence waits between, letting OS communicate with GPU.
- Fence waits aren't just CPU-GPU sync primitives — also TDR-avoidance: GPU writes timestamp at fence signal, driver confirms GPU still alive.
- Batch to single command buffer + single fence wait has lower CPU overhead (1 alloc/free vs 405). Best is **chunked batching**: chunk work so each < 2s but chunk count << resource count.

**Chunking strategy comparison**:

| Strategy | CPU overhead | GPU burst | TDR? |
|------|----------|----------|-------|
| Individual builds (405 submits) | very high (405× fence/buffer) | short (~12ms) | no |
| Full batch (1 submit) | low (1× fence/buffer) | long (~5s) | **yes** |
| **Chunked (64/chunk, 7 submits)** | **low (7× fence/buffer)** | **medium (~700ms)** | **no** |

### 34.3 Layer 3 (root cause): combined index buffer index offset causes GPU out-of-bounds read

**Symptom**: after chunking device lost persists, but execution time unchanged (still ~5s GPU), so not TDR.

**Root cause**: `build_pt_scene` concatenates all instances' vertices and indices into one combined buffer, indices offset by `vertex_base` (each instance's start in combined vertex array). Then calls `BlasEntry::build_at(vertex_addr=address at combined offset, index_addr=address at combined index offset, vertex_count=instance vertex count)`.

In BLAS geometry:
- Vertex buffer starts at `vaddr = vbase + vertex_base × stride` (instance first vertex)
- Index buffer contains `original_index + vertex_base` (offset values)
- GPU computes vertex position: `vaddr + (original_index + vertex_base) × stride`
- = `(vbase + vertex_base × stride) + (original_index + vertex_base) × stride`
- = `vbase + (2 × vertex_base + original_index) × stride` ← **double offset!**

For later instances (large `vertex_base`), `2 × vertex_base + original_index` easily exceeds total vertex count → out-of-bounds GPU read → GPU page fault → device lost!

Old `queue_wait_idle` code never actually executed BLAS builds (returned immediately), so bug never triggered. Fence fix exposed it.

**Fix**: set `vertex_addr` to combined buffer base (`vbase`), set `vertex_count` (as `max_vertex`) to total vertex count. Since index values already relative to `vbase` (`vertex_base + original_index`), GPU computes correctly:
`vbase + (vertex_base + original_index) × stride` ✓

**Lessons**:
- **`queue_wait_idle` returning early hides GPU errors**. Old code could run weeks without discovering BLAS addressing bug because GPU never actually executed builds — submit swallowed, validation only complainer. Switching to correct fence sync, all accumulated GPU errors instantly become device lost.
- **"Fence fix causes new problems" usually means old bug exposed**. Sequential symptoms (validation error → longer time → device lost) are same root cause (index offset) surfacing through different sync layers.
- **Validation error vs device lost**: validation error signals API misuse (freeing in-use buffer) but not necessarily direct crash. Device lost = GPU unrecoverable. Fix validation errors first, then device lost.
- When writing index-concatenation loops check relative-offset vs absolute-base consistency. Especially when same buffer interpreted differently by different GPU units (BLAS build, shader memory access), each consumer's offset convention must be independently verified.
- `BlasEntry::build_at` design (takes `vertex_addr` but infers `max_vertex` from `vertex_count`) makes caller responsible for correct base. If caller passes offset address, `max_vertex` must relative to that offset (instance local `vertex_count - 1`); if combined base, `max_vertex` must total vertex count. **This "relative to what?" convention isn't documented in API**, mismatch between two consumers. Better design: `BlasBuildParams` require explicit `max_vertex` (not `vertex_count`), forcing caller to think.

### 34.4 Investigation logic

```
Device lost
├─ Validation: vkFreeCommandBuffers in-use
│  └─ queue_wait_idle returns early → replace with fence ✓
│
└─ Device lost (still after fence fix)
   ├─ Time ~5s → suspect TDR
   │  ├─ Single batch submit → still ~5s + device lost
   │  └─ Chunk 64/batch → time unchanged + device lost still
   │     └─ Not TDR, GPU crashes during execution
   │
   └─ BLAS addressing error → index double offset ✓
      └─ Old code never actually executed BLAS (queue_wait_idle early return),
         bug hidden. Fence fix exposed it.
```

### 34.5 Summary

| Layer | Symptom | Root cause | Fix |
|----|------|------|------|
| Sync | `vkFreeCommandBuffers in-use` validation error | `queue_wait_idle` returns early | Replace with dedicated fence |
| GPU time | ~5s execution after fence fix | 405 individual submits, 12ms each | Chunked batch (64/chunk, 7 submits) |
| GPU crash | persistent device lost | BLAS index double `vertex_base` | `vertex_addr` uses combined base |

Each layer could be mistaken for root cause, actually only upper-layer symptom. True bug (index offset) hidden under old sync code. **Fence fix is diagnostic tool, not final fix** — it revealed real problem.

## 35. Per-frame descriptor updates need `UPDATE_AFTER_BIND` flag

**Background**: `PathTracePass` calls `update_ds()` each frame writing all descriptors (bindings 0-7), because geometry / material / output resources change between frames (TLAS rebuild, accum image resize, etc). Descriptor set layout created without `UPDATE_AFTER_BIND`, validation error:

```
vkUpdateDescriptorSets(): dstBinding (0) was created with VkDescriptorBindingFlags(0),
but VkDescriptorSet ... is in use by VkCommandBuffer ...
```

**Root cause**: PathTracePass descriptor set created in `new()` with default flag (= no update-after-bind). But `execute()` calls `update_ds()` each frame while previous frame's command buffer may still be in flight (pending), writing descriptor set currently referenced by GPU violates spec.

**Fix**:
- Add `UPDATE_AFTER_BIND | PARTIALLY_BOUND` to descriptor set layout binding flags
- Add `UPDATE_AFTER_BIND_POOL` flag to layout creation
- Add `UPDATE_AFTER_BIND` flag to pool creation

```rust
// modify before

let ds_layout = device.create_descriptor_set_layout(
    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)?;

// modify after
let binding_flags = [
    vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
        | vk::DescriptorBindingFlags::PARTIALLY_BOUND,
    // ... each binding same
];
let mut flags_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
    .binding_flags(&binding_flags);
let ds_layout = device.create_descriptor_set_layout(
    &vk::DescriptorSetLayoutCreateInfo::default()
        .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
        .bindings(&bindings)
        .push_next(&mut flags_info), None)?;
let ds_pool = device.create_descriptor_pool(
    &vk::DescriptorPoolCreateInfo::default()
        .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)  // pool too
        .max_sets(1).pool_sizes(&pool_sizes), None)?;
```

**Which descriptor sets need `UPDATE_AFTER_BIND`**:
| Usage pattern | UAB needed? | Alternative |
|----------|-----------|----------|
| Written once at init, read-only after | No | — |
| Same set written each frame but one frame in flight (single buffer) | No | If `wait_idle` or fence guarantees |
| Same set written each frame, multiple frames parallel (double / triple buffer) | **Yes** | Or allocate new set per frame (costly) |
| Only updating bindings with `UPDATE_AFTER_BIND` | only that binding | Other bindings still restricted |
| Using `PARTIALLY_BOUND` (unbound descriptor slots) | recommended | — |

**Lessons**:
- Descriptor binding flags aren't optional polish — mandatory when update pattern involves "writing set in use by GPU".
- `BindlessTextureTable` already uses these flags correctly. `PathTracePass` (and future temporal-accum / per-frame-rebuild passes) must follow same pattern.
- Signal: if `execute()` calls `update_descriptor_sets()`, almost certainly needs `UPDATE_AFTER_BIND`.

## 36. `fix_spv` omission: all compute shaders importing `path_integrator.slang` also need ArrayStride fix

**Background**: `compile.sh` calls `fix_spv` on `scene_frag.frag.spv` (line 130), but three compute shaders (`pt_render.comp.spv`, `gi_bake.comp.spv`, `gi_render.comp.spv`) didn't call `fix_spv` after compile. `pt_render.comp.spv` load triggers SPIR-V validation error:

```
vkCreateShaderModule(): pCreateInfo->pCode (spirv-val produced an error):
Invalid explicit layout decorations on type for operand '580[%580]'
  %bindlessSrvs = OpVariable %_ptr_UniformConstant__runtimearr_579 UniformConstant
```

**Root cause**: `path_integrator.slang` declares bindless texture / sampler runtime arrays:
```slang
[[vk::binding(0, 1)]]
SamplerState globalSamplers[];
[[vk::binding(1, 1)]]
Texture2D bindlessSrvs[];
```
Slang compiler generates illegal `OpDecorate ArrayStride <n>` decorations for these runtime arrays' opaque element types. `fix_spirv.py` can strip them, but:

- `scene_frag.slang` directly declares same bindings (line 78-85), fixed by `fix_spv` ✅
- `pt_render.slang` indirectly declares same bindings via `import path_integrator`, no `fix_spv` ❌

When `compile.sh` originally added `fix_spv` it only considered rasterizer fragment shader, missed later-added compute shaders.

**Fix**: add `fix_spv` calls for all compute shaders importing `path_integrator`:
```bash
# compile.sh line 152-168
fix_spv "$OUT/gi_bake.comp.spv"
fix_spv "$OUT/gi_render.comp.spv"
fix_spv "$OUT/pt_render.comp.spv"
```

> **Deprecated (2026-11)**: Slang 2026.21 no longer generates `ArrayStride` for opaque runtime arrays, `fix_spirv.py` and all `fix_spv` calls deleted. CI and local both use 2026.21, `spirv-val` gate at end of `compile.sh` catches regressions. Section kept as history, judgment rules no longer apply.

**Lessons** (only for Slang ≤ 2026.13; unnecessary since 2026.21):
- `fix_spv` isn't for specific filenames, but **any SPIR-V module containing bindless runtime array declarations**. When adding new slang files declaring or importing opaque runtime arrays like `SamplerState[]` / `Texture2D[]`, must add `fix_spv`.
- Simple rule: if `.slang` contains `globalSamplers[]` or `bindlessSrvs[]` declarations, or imports module containing them (`path_integrator.slang`), needs `fix_spv` after compile.
- Slang runtime-array ArrayStride bug affects all bindless-texture compute shaders, not just fragment. §13 fragment reproduction, §33 slangc data corruption — third variant: compiler-generated SPIR-V rejected by `spirv-val`.

## 37. `UPDATE_AFTER_BIND` requires matching device sub-features per descriptor type

**Background**: §35 added `UPDATE_AFTER_BIND | PARTIALLY_BOUND` to `PathTracePass` descriptor set layout, `vkCreateDescriptorSetLayout` error:

```
pBindingFlags[0] includes VK_DESCRIPTOR_BINDING_UPDATE_AFTER_BIND_BIT
but pBindings[0].descriptorType is VK_DESCRIPTOR_TYPE_STORAGE_IMAGE
but descriptorBindingStorageImageUpdateAfterBind was not enabled.
```

Same error repeated for bindings 0-7, three descriptor types each map to different feature bits.

**Root cause**: `UPDATE_AFTER_BIND` is a binding-level flag, but Vulkan requires enabling corresponding sub-feature per descriptor **type** at device creation. Only layout/pool flag insufficient — Vulkan 1.2 `PhysicalDeviceVulkan12Features` has fine-grained `descriptorBinding<Type>UpdateAfterBind` fields, each controls update-after-bind for one descriptor type.

**Fix**: at device creation in `context.rs`, enable corresponding features for three descriptor types used by `PathTracePass`:

| Descriptor type | Vulkan feature field | Struct |
|-----------|-------------------|-----------|
| `VK_DESCRIPTOR_TYPE_STORAGE_IMAGE` | `descriptor_binding_storage_image_update_after_bind` | `PhysicalDeviceVulkan12Features` |
| `VK_DESCRIPTOR_TYPE_STORAGE_BUFFER` | `descriptor_binding_storage_buffer_update_after_bind` | `PhysicalDeviceVulkan12Features` |
| `VK_DESCRIPTOR_TYPE_ACCELERATION_STRUCTURE_KHR` | `descriptor_binding_acceleration_structure_update_after_bind` | `PhysicalDeviceAccelerationStructureFeaturesKHR` |

```rust
// in descriptor_indexing block
vk12.descriptor_binding_storage_image_update_after_bind = vk::TRUE;
vk12.descriptor_binding_storage_buffer_update_after_bind = vk::TRUE;

// in acceleration_structure block
accel_features.descriptor_binding_acceleration_structure_update_after_bind = vk::TRUE;
```

**Why didn't `BindlessTextureTable` trigger this?**

`BindlessTextureTable` has only two descriptor types: `SAMPLER` and `SAMPLED_IMAGE`. `SAMPLED_IMAGE` needs `descriptor_binding_sampled_image_update_after_bind`, already enabled in `if rt_caps.descriptor_indexing` block (added for bindless originally). So bindless table fine.

`PathTracePass` additionally uses `STORAGE_IMAGE`, `STORAGE_BUFFER`, `ACCELERATION_STRUCTURE_KHR`, each needing separate update-after-bind sub-feature. Vulkan feature design intentionally type-granular — sampled image can support UAB while storage image not, if app only needs bindless textures.

**Lessons**:
- `UPDATE_AFTER_BIND` needs **three layers** enabled, none optional:
  1. **Device features**: `descriptorBinding<Type>UpdateAfterBind` in `PhysicalDeviceVulkan12Features` or corresponding extension feature struct
  2. **Layout flag**: `DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL` + `UPDATE_AFTER_BIND_BIT` in `DescriptorSetLayoutBindingFlagsCreateInfo`
  3. **Pool flag**: `DescriptorPoolCreateFlags::UPDATE_AFTER_BIND`
- When adding descriptor type to update-after-bind set, first check corresponding `descriptorBinding<Type>UpdateAfterBind` enabled at device creation — most common omission.
- `BindlessTextureTable::required_features()` returns its needed feature set, but `PathTracePass` has no `required_features()`. In practice each module needing special device features should expose `required_features()`, aggregated at device creation.

## MIS weights only valid when "both strategies can sample the same light source"

`path_integrator.slang` multi-light NEE weights used to be:

```slang
float p_nee_al = 1.0 / (4.0 * PI * num_lights);   // treated as per-steradian pdf
total += brdf * L * p_nee_al / (p_nee_al*p_nee_al + p_brdf*p_brdf);
```

Point / spot lights are **analytic delta lights**: not in TLAS, BRDF sampling never hits them, so NEE is only strategy — no second pdf to balance. Worse `1/(4πN)` is **light-selection probability**, dimension "per light", while `p_brdf` is "per steradian"; mixing them in power heuristic scales contribution by wrong-dimension pdf: N=1 point light keeps only `p²/(p²+p_brdf²) ≈ 6%` energy.
so NEE is only strategy — no second pdf to balance. Worse that `1/(4πN)` is
**light-selection probability**, dimension "per light", while `p_brdf` is "per steradian"; mixing them in same
power heuristic scales contribution with wrong-dimension pdf: N=1 point light keeps only
`p²/(p²+p_brdf²) ≈ 6%` energy.

For MIS must first convert sampling-strategy probabilities to **same measure** (usually solid angle): area sampling → solid angle multiply Jacobian `d² / |cos_light|` (emissive triangle NEE does this). Delta lights have no area, no Jacobian — correct weight is 1.
Area sampling → solid angle multiply Jacobian `d² / |cos_light|` (emissive triangle NEE does this).
Delta lights have no area, no Jacobian — correct weight is 1.

**Lessons**:
- Before adding MIS ask: can both strategies sample same light? Delta lights (point, directional, center-point sampling approximating area lights) can't MIS with BRDF sampling.
- pdf dimensions must align: discrete probability (light selection `1/N`), area pdf, solid-angle pdf differ; mixing silently darkens / brightens, compiler won't error.
- Such errors only appear with "analytic lights + NEE on", visually like "lights not bright enough", easily mistaken for tonemap or exposure issue.
  mixing silently darkens / brightens, compiler won't error.
- ( See original for full MIS derivation )
  easily mistaken for tonemap or exposure issue.

## VkApplicationInfo.api_version limits Vulkan 1.2+ core function symbol export

**Problem**: headless / offscreen rendering with ray tracing, `PathTracePass::set_geometry` -> `build_pt_scene` triggers ash panic:
`Unable to load get_buffer_device_address` (crash at `ash::tables::DeviceFnV1_2::load_erased`).

**Root cause**:
When creating Vulkan instance, `VkApplicationInfo.apiVersion` hardcoded to `vk::API_VERSION_1_1`. Per spec, when instance API version declared 1.1, strict drivers (e.g. NVIDIA Windows) return `NULL` from `vkGetDeviceProcAddr` for Vulkan 1.2 core entry points without `KHR` suffix (`vkGetBufferDeviceAddress`).
Per Vulkan spec, when instance API version declared 1.1, strict drivers (e.g. NVIDIA Windows) return `NULL` from `vkGetDeviceProcAddr` for Vulkan 1.2 core entry points without `KHR` suffix (e.g. `vkGetBufferDeviceAddress`).
`ash::Device` `get_buffer_device_address` directly calls `self.device_fn_1_2.get_buffer_device_address` (1.2 table), null pointer enters ash-generated panic stub.

**Solution and lessons**:
- Use `entry.try_enumerate_instance_version()` to probe loader max supported instance version.
- When loader supports, request `vk::API_VERSION_1_2` (or no less than 1.2), so driver safely exports Vulkan 1.2 core function pointers.
- Never write "target is Vulkan 1.2" in comments while passing `API_VERSION_1_1` in `ApplicationInfo`.

## Alignment risk of `include_bytes!` embedded SPIR-V bytes

**Problem**: calling `bytemuck::cast_slice::<u8, u32>(SPV)` on SPIR-V bytes embedded by `include_bytes!` macro panics at runtime in some build configs:
`bytemuck: TargetAlignmentGreaterAndInputNotAligned`.

**Root cause**:
Rust `include_bytes!` embeds binary into read-only data (`.rodata`), pointer only 1-byte aligned. If embedded start address isn't 4-byte aligned, `bytemuck::cast_slice` alignment check panics.

**Solution and lessons**:
- Render pipeline loading SPIR-V uniformly calls `crate::shader::load_shader_module`.
- `load_shader_module` explicitly checks pointer alignment; if unaligned, safely regroups via `as_chunks::<4>` into `u32`, avoiding UB and alignment panic.

## RTXGI SHARC VRAM barriers and 64-bit buffer atomics

**Experience**:
- SHARC hash table based on 64-bit spatial hash keys (`uint64`), physical device must enable `PhysicalDeviceVulkan12Features.shader_buffer_int64_atomics` at logical device creation.
- When SHARC allocates VRAM, first frame must zero-init via `cmd_fill_buffer`, then `TRANSFER -> COMPUTE_SHADER` memory barrier (`TRANSFER_WRITE -> SHADER_READ | SHADER_WRITE`) sync, preventing uninitialized dirty data recognized as valid spatial voxels.
- After `SharcResolvePass::execute` compute writes, must issue `COMPUTE_SHADER -> COMPUTE_SHADER | FRAGMENT_SHADER` memory barrier before subsequent ray-tracing / denoise passes, ensuring spatial-hash radiance cross-frame resolve correctly visible.

## Path-tracing temporal accumulation: convenience wrapper layers must not hardcode `pt_accum_dirty: true`

**Problem**: headless offscreen render or external call `GraphRenderer::render_frame_with_overrides`, path-traced image is high-freq random noise every frame, increasing sample iterations doesn't converge.

**Root cause**:
When `GraphRenderer` convenience method constructs `FramePacket`, `pt_accum_dirty` field hardcoded `true`.
`PathTracePass::execute` each time detects `pt_accum_dirty: true`, calls `clear_accum_images` and resets `sampleCount` to 0, so designed temporal accumulation discarded unconditionally each frame.

**Lessons**:
- State machines / packet wrappers must not abuse hardcoded reset flags, default to `false`, set `true` only on camera move, scene change or explicit invalidation.
- When investigating multi-frame accumulation pipelines, first log current frame `sample_count` and accumulation weight; if always 1, immediately conclude reset flag or dirty state anomaly.

## Async asset streaming and screenshot disk-write timing race

**Problem**: set screenshot plan (`ScreenshotPlan`) to auto-capture at frame 90, output image only skybox or pure black, core 3D meshes all missing.

**Root cause**:
Large 3D assets (e.g. Crytek Sponza) usually loaded and deserialized async by engine resource system on background thread. Main render thread loops fast from frame 1; at frame 90, background IO hasn't yet attached 103 sub-mesh entities to ECS, so screenshot logic fires early on empty scene and marks complete exit.

**Lessons**:
- Automated screenshots and goldenframe tests can't rely only on fixed frame count.
- Must inject content-ready guard into capture plan: `plan.fires_on(frame_count + 1) && (!packet.draw_items.is_empty() || frame_count >= 300)`, ensuring draw commands truly ready before capture and disk write.

## Slang matrix multiply bytecode (RowMajor and OpVectorTimesMatrix)

**Problem**: observed path tracing / normal output differs from rasterization, suspected matrix not transposed, so called `mat_transpose` on transform matrices in Host code, causing meshes all flattened or projection flipped.

**Root cause**:
Disassembling Slang-generated SPIR-V with `spirv-dis` revealed Slang-generated push constants default to `RowMajor` decoration, and matrix-vector multiply compiles to `OpVectorTimesMatrix %vec %mat`.
On Vulkan hardware, $\vec{v} \cdot M_{\text{spirv}} = \vec{v} \cdot (M_{\text{rust}})^T = M_{\text{rust}} \cdot \vec{v}$, its memory-read logic exactly cancels Rust column-major array, mathematically equivalent to standard column-vector multiply. Extra transpose on Host causes Double Transpose.

**Lessons**:
- For shader matrix layout issues, never blindly guess by adding/removing `transpose()`, must disassemble corresponding `.spv` to see exact instructions.
- Only by accurately comparing shader memory decorations (`RowMajor`/`ColMajor`) and vector-multiply instructions (`OpVectorTimesMatrix` / `OpMatrixTimesVector`) can correctly derive Host-side data format.

## Investigation discipline case: Sponza camera move and M key (read call sites first, forbid cross-crate archaeology)

**Problem**: after locking mouse WASD doesn't respect lock state, `M` key no response at all.

**Actual mishap review and reflection**:
- **Wrong investigation path**: first `git blame` through commit history, read winit 0.30 `refresh_os_cursor` impl, write PowerShell probe for `GetClipCursor`, run `sponza.exe` background capturing logs, repeatedly compress context. None changed conclusion, just turned a simple conditional-judgment problem into blind cross-crate archaeology.
- **Correct localization only three steps** (each reads one function, directly look at code through grep-hit call sites):
  1. `projects/demos/sponza/src/tuning_ui.rs` `on_key_pressed` → `Key::Other(VK_M) if !self.locked`, but window auto-locks on focus, so key never received during gameplay.
  2. `crates/prism-engine/src/scene/systems/camera.rs` `camera_controller_system` → `look_active` only blocked WASD, mouse look still had `|| right-button` backdoor.
  3. `crates/prism-platform/src/window.rs` `grab_pointer` → grab failure only `log::warn`, still sets `locked` true, so "locked" may be false state.
- **Single source of truth for state lesson**: pointer lock used to scatter across platform-window atomic, `InputManager::pointer_locked`, project-hook own `self.locked` — three copies, each updated separately inevitably desync. **Key state deciding "who owns input" must have single truth in platform layer, other consumers re-read each frame** (e.g. `InputManager::sync_pointer_lock`).

**Lessons**:
- Bug-report behavior almost always corresponds to one `if` condition or one state write in code. Facts already hardcoded in code must never be repeatedly "proven" via probes, blame or external crate source diving.
- One bug reads only its traversal chain; stop after reading grep-hit files, don't roam entire repo scanning second time.
- State bugs must first confirm whether boolean has multiple copies and caches in system.
