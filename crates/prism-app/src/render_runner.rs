//! Render thread---receives [`FramePacket`] and overlay messages from main thread,
//! and drives [`GraphRenderer`] (begin_frame -> execute -> present).
//!
//! Render thread **asynchronously** started by [`App`](crate::app::App) in `resumed`;
//! main thread only creates window (fast), render thread internally builds [`GraphRenderer`]
//! (~hundreds ms, but no longer blocks window events). Runs until main thread sets `running` flag to `false`.
//!
//! Asset parsing also completes GPU side in this thread: main thread CPU side produces [`AssetResolveRequest`]
//! delivered via channel, this thread takes each frame and hands to `GraphRenderer::apply_asset_requests`.

use std::sync::Arc;
use std::time::Instant;

use prism_engine::dirty_router::{draw_items_signature, DirtyRouter};
use prism_engine::ecs::Entity;
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::FramePacket;
use prism_platform::SendWindowHandles;
use prism_render::asset_bridge::{AssetResolveRequest, AssetResolveResult};
use prism_render::frame_stats::{FrameStats, FrameStatsSummary};
use prism_render::SwapchainOverlay;
use prism_render::{FrameInput, FrameUBOData, GraphRenderer};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};

use crate::render_shared::{FrameImage, RenderShared, RenderStats};

/// `Send` wrapper of process-level raw window handle.
///
/// `RawDisplayHandle`/`RawWindowHandle` itself contains `NonNull<c_void>`
/// (Wayland variant) statically judged non-`Send` by Rust, cannot directly cross threads. But this
/// engine only runs on Windows/Android, its actual variant is process-level pointer value (Win32's
/// `HWND` / Android `ANativeWindow` pointer, display handle is empty struct),
/// using on any thread to create Vulkan surface is safe; and window lifecycle covers whole render
/// thread, so explicitly marking `Send` here is valid.
/// Re-wrap [`RawDisplayHandle`]/[`RawWindowHandle`] extracted by main thread into
/// `HasDisplayHandle`/`HasWindowHandle`, for `GraphRenderer::new` inside render thread
/// use. winit's `Window::window_handle()` on **non-creation thread** returns error
/// (handle unavailable), so must extract raw handle on main thread then pass across threads---
/// raw HWND is process-level value, safe on any thread to create Vulkan surface.
struct RawHandleBridge<'a> {
    display: &'a RawDisplayHandle,
    window: &'a RawWindowHandle,
}

impl HasDisplayHandle for RawHandleBridge<'_> {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(*self.display) })
    }
}

impl HasWindowHandle for RawHandleBridge<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(*self.window) })
    }
}

/// Frame counter smoothing factor (lower value smoother).
const FPS_ALPHA: f32 = 0.05;

/// External overlay factory: provided by main thread before render thread starts, returns [`SwapchainOverlay`].
pub type OverlayFactory = Box<dyn Fn() -> Box<dyn SwapchainOverlay> + Send>;

/// Render thread present target.
pub enum RenderTarget {
    /// Real window + swapchain (game / launcher).
    Surface {
        handles: SendWindowHandles,
        extensions: Vec<String>,
        vsync: bool,
    },
    /// Native surface supplied by an embedding host. The host owns the
    /// window; the renderer owns only the Vulkan surface and swapchain.
    NativeSurface {
        handles: SendWindowHandles,
        extensions: Vec<String>,
        vsync: bool,
    },
    /// Windowless offscreen image (editor viewport). Frame requested by host via [`RenderShared::request_frame`]
    /// request, render thread reads back to CPU then puts into [`RenderShared::frame`].
    Offscreen { width: u32, height: u32 },
}

/// Render thread entry point.
///
/// Unlike old implementation, this function builds [`GraphRenderer`] **inside render thread**:
/// - `handles` are raw handles extracted by main thread on window creation thread ([`SendWindowHandles`])
///   -- `Window::window_handle()` fails on non-creation thread, so must extract on main thread then pass.
/// - `extensions` are Vulkan instance extensions computed by main thread (depend on window display handle).
/// - `scene_env` is IBL data of scene declarative lighting (None -> cheap empty IBL, no convolution).
/// - `warmup` whether to warm up all GPU pipelines before first frame.
/// - `vsync` whether to request vsync present mode (from `WindowConfig.vsync`).
/// - `overlay_factory` if present, install external overlay immediately after build completes.
///
/// Thus `resumed()` can return immediately after window built; window events (close / move / resize)
/// no longer blocked by renderer init. Loop: build renderer -> take packet/asset/overlay messages
/// -> build input -> begin/execute/present. Exit when `shared.running` becomes `false`.
pub fn render_thread_main(
    shared: Arc<RenderShared>,
    target: RenderTarget,
    scene_env: Option<Vec<u8>>,
    warmup: bool,
    overlay_factory: Option<OverlayFactory>,
    shadow_map_size: u32,
    shadow_cascade_count: u32,
) {
    let mut dirty_router = DirtyRouter::new();
    let mut graph_report = crate::render_graph_report::ReportWriter::from_env();

    log::info!("Render thread started");
    prism_engine::util::timing::event("render::thread_started", "");

    let create_span = prism_engine::util::timing::span("render::create_renderer");
    let created = match &target {
        RenderTarget::Surface {
            handles,
            extensions,
            vsync,
        } => {
            let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
            let handle_bridge = RawHandleBridge {
                display: &handles.display,
                window: &handles.window,
            };
            GraphRenderer::new_with_vsync_and_shadow_size(
                extensions_ref,
                &handle_bridge,
                &handle_bridge,
                None,
                *vsync,
                shadow_map_size,
                shadow_cascade_count,
            )
        }
        RenderTarget::NativeSurface {
            handles,
            extensions,
            vsync,
        } => {
            let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
            let handle_bridge = RawHandleBridge {
                display: &handles.display,
                window: &handles.window,
            };
            GraphRenderer::new_with_vsync_and_shadow_size(
                extensions_ref,
                &handle_bridge,
                &handle_bridge,
                None,
                *vsync,
                shadow_map_size,
                shadow_cascade_count,
            )
        }
        RenderTarget::Offscreen { width, height } => GraphRenderer::offscreen_new_with_shadow_size(
            *width,
            *height,
            None,
            shadow_map_size,
            shadow_cascade_count,
        ),
    };
    drop(create_span);
    let mut renderer = match created {
        Ok(r) => r,
        Err(e) => {
            let message = format!("failed to create renderer: {e:#}");
            log::error!("Render thread: {message}");
            shared.fail_render(message);
            return;
        }
    };
    if !prism_engine::asset_paths::strict_pak_only() {
        if let Some(prt_path) = prism_engine::asset_paths::asset_path("gi/prt_volume.prtbin") {
            if prt_path.is_file() {
                renderer.load_prt_volume_file(&prt_path);
            }
        }
    }

    shared.mark("renderer_built");
    prism_engine::util::timing::event("render::renderer_built", "");
    log::info!("Render thread: renderer built");

    if warmup {
        let warmup_span = prism_engine::util::timing::span("render::warmup_pipelines");
        if let Err(e) = renderer.warmup_pipelines() {
            log::warn!("Render thread: pipeline warmup failed (continuing): {e:#}");
        }
        drop(warmup_span);
        shared.mark("warmup_done");
    }

    if let Some(bytes) = scene_env {
        log::info!("scene lighting: building IBL environment (with convolve)");
        let env_span = prism_engine::util::timing::span("render::set_environment_ibl");
        if let Err(e) = renderer.set_environment(Some(bytes)) {
            log::warn!("Render thread: failed to set scene environment: {e:#}");
        }
        drop(env_span);
    } else {
        log::info!("scene lighting: no IBL environment — cheap empty IBL (no convolve)");
    }

    if let Some(factory) = overlay_factory {
        renderer.set_external_overlay(factory());
    }

    let mut smoothed_fps = 0.0f32;
    let mut frame_count: u64 = 0;
    let mut frame_stats = FrameStats::new(8192);
    let mut frame_stats_summary = FrameStatsSummary::default();
    let mut first_frame_presented = false;
    let mut first_packet_received = false;
    let mut consecutive_errors: u32 = 0;
    let screenshot = crate::screenshot::ScreenshotPlan::from_env();
    let mut screenshot_done = false;
    let mut bench =
        crate::bench_report::BenchPlan::from_env().map(crate::bench_report::BenchCollector::new);
    let mut bench_report_written = false;

    while shared.running.load(std::sync::atomic::Ordering::Relaxed) {
        if shared.take_pt_reset() {
            renderer.request_pt_reset();
        }

        let requests = shared.take_asset_requests();
        if !requests.is_empty() {
            let (entities, stripped): (Vec<Entity>, Vec<AssetResolveRequest>) =
                requests.into_iter().unzip();
            let results = renderer.apply_asset_requests(stripped);
            let paired: Vec<(Entity, AssetResolveResult)> =
                entities.into_iter().zip(results).collect();
            shared.push_asset_results(paired);
        }

        let packet = shared.take_packet();
        let overlay_messages = shared.take_overlay_messages();
        for msg in overlay_messages {
            renderer.apply_overlay_message(msg);
        }

        let renderer_messages = shared.take_renderer_messages();
        for msg in renderer_messages {
            msg(&mut renderer);
        }

        let packet = match packet {
            Some(packet) => {
                if !first_packet_received {
                    first_packet_received = true;
                    prism_engine::util::timing::event(
                        "render::first_packet_received",
                        &format!(
                            "draws={} camera={} extent={}x{}",
                            packet.draw_items.len(),
                            packet.scene.has_camera,
                            renderer.extent().width,
                            renderer.extent().height
                        ),
                    );
                    log::info!(
                        "first render packet received: draws={}, camera={}, gizmos={}, extent={}x{}",
                        packet.draw_items.len(),
                        packet.scene.has_camera,
                        packet.gizmos.len(),
                        renderer.extent().width,
                        renderer.extent().height,
                    );
                }
                packet
            }
            None => {
                let guard = shared.packet.lock().unwrap();
                if guard.is_none() && shared.running.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = shared
                        .packet_ready
                        .wait_timeout(guard, std::time::Duration::from_millis(50))
                        .unwrap();
                }
                continue;
            }
        };

        let frame_start = Instant::now();

        let frame_plan = screenshot.as_ref().filter(|plan| {
            !screenshot_done
                && plan.fires_on(frame_count + 1)
                && (!packet.draw_items.is_empty()
                    || !packet.sprite_items.is_empty()
                    || renderer.is_headless()
                    || frame_count >= 300)
        });
        let presented = match render_one_frame(
            &mut renderer,
            &packet,
            &mut dirty_router,
            &packet.settings,
            frame_plan,
        ) {
            Ok(presented) => {
                consecutive_errors = 0;
                if !presented {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                presented
            }
            Err(e) => {
                consecutive_errors += 1;
                let text = format!("{e:#}");
                let fatal = text.contains("device has been lost")
                    || text.contains("DEVICE_LOST")
                    || text.contains("surface has been lost")
                    || consecutive_errors >= 3;
                if fatal {
                    let message =
                        format!("render failed after {consecutive_errors} attempt(s): {text}");
                    log::error!("Render thread: {message}");
                    shared.fail_render(message);
                    break;
                }
                log::error!("Render thread error: {e:#}");
                false
            }
        };
        if presented && !first_frame_presented {
            first_frame_presented = true;
            log::info!(
                "first frame presented: extent={}x{}, draws={}, camera={}, target={}",
                renderer.extent().width,
                renderer.extent().height,
                packet.draw_items.len(),
                packet.scene.has_camera,
                if renderer.is_headless() {
                    "offscreen"
                } else {
                    "surface"
                },
            );
            shared.mark("first_frame");
            shared
                .renderer_ready
                .store(true, std::sync::atomic::Ordering::Relaxed);
            prism_engine::util::timing::event(
                "render::first_frame",
                &format!(
                    "draws={} extent={}x{}",
                    packet.draw_items.len(),
                    renderer.extent().width,
                    renderer.extent().height
                ),
            );
            shared.print_startup_report();
            prism_engine::util::timing::print_report();
        }

        if frame_plan.is_some() {
            screenshot_done = true;
            if std::env::var("PRISMA_EXIT_ON_SCREENSHOT").is_ok() {
                log::info!("PRISMA_EXIT_ON_SCREENSHOT: screenshot finished, exiting process");
                std::process::exit(0);
            }
        }

        let elapsed = frame_start.elapsed();
        let frame_time_ms = elapsed.as_secs_f32() * 1000.0;

        frame_count += 1;
        if frame_count == 1 {
            smoothed_fps = 1.0 / elapsed.as_secs_f32().max(1e-6);
        } else {
            let instant_fps = 1.0 / elapsed.as_secs_f32().max(1e-6);
            smoothed_fps = FPS_ALPHA * instant_fps + (1.0 - FPS_ALPHA) * smoothed_fps;
        }

        let pt_count = renderer.pt_frame_count();
        let gpu_timings = renderer.gpu_timings().clone();

        frame_stats.push_frame(
            frame_time_ms,
            gpu_timings
                .supported
                .then_some(gpu_timings.frame_or_total_ms()),
        );
        if frame_count.is_multiple_of(30) {
            frame_stats_summary = frame_stats.summary();
        }

        if let Some(collector) = bench.as_mut() {
            let complete = collector.sample(
                frame_time_ms,
                gpu_timings
                    .supported
                    .then_some(gpu_timings.frame_or_total_ms()),
            );
            if complete && !bench_report_written {
                bench_report_written = true;
                let extent = renderer.extent();
                let metadata = collector.metadata(
                    renderer.context().adapter_name(),
                    Some((extent.width, extent.height)),
                );
                match collector.write(&metadata) {
                    Ok(()) => log::info!(
                        "benchmark report written: {} ({} samples, {} warm-up frames)",
                        collector.plan().path.display(),
                        collector.samples(),
                        collector.plan().warmup_frames,
                    ),
                    Err(e) => log::error!(
                        "benchmark report write failed ({}): {e}",
                        collector.plan().path.display()
                    ),
                }
            }
        }

        if graph_report.is_enabled() {
            graph_report.maybe_write_with_timings(
                &renderer.graph().snapshot(),
                packet.settings.to_report_settings(),
                frame_count,
                Some(&gpu_timings),
                Some(renderer.render_stats()),
                renderer.draw_call_report(
                    &packet.draw_items,
                    &packet.sprite_items,
                    &packet.particle_items,
                ),
                renderer.gpu_history().to_vec(),
            );
        }

        shared.set_render_stats(RenderStats {
            render_seq: frame_count,
            frame_time_ms,
            fps: smoothed_fps,
            pt_frame_count: pt_count,
            gpu_timings: gpu_timings.supported.then_some(gpu_timings),
            frame_stats: frame_stats_summary.clone(),
        });

        if renderer.is_headless() {
            match renderer.offscreen_extent() {
                Some(extent) => {
                    if let Ok(Some(pixels)) = renderer.poll_offscreen_readback() {
                        *shared.frame.lock().unwrap() = Some(FrameImage {
                            width: extent.width,
                            height: extent.height,
                            pixels,
                        });
                    }
                    if shared
                        .frame_requested
                        .swap(false, std::sync::atomic::Ordering::Relaxed)
                    {
                        if let Err(e) = renderer.copy_offscreen_to_readback_async() {
                            log::warn!("Render thread: frame readback submit failed: {e:#}");
                        }
                    }
                }
                None => log::warn!("Render thread: frame requested but no offscreen target"),
            }
        }

        shared.recycle_packet(packet);
    }

    log::info!("Render thread exiting — destroying renderer");
    drop(renderer);
}

/// Render one frame: build [`FrameInput`] from a [`FramePacket`] then drive
/// the five-phase API (begin → prepare → execute → present → end).
fn render_one_frame(
    renderer: &mut GraphRenderer,
    packet: &FramePacket,
    dirty_router: &mut DirtyRouter,
    settings: &RenderSettings,
    screenshot: Option<&crate::screenshot::ScreenshotPlan>,
) -> anyhow::Result<bool> {
    let scene = &packet.scene;
    let draw_items = &packet.draw_items;

    let draw_items_dirty = dirty_router.draw_items_dirty(draw_items_signature(draw_items));
    let pt_bounces_dirty = dirty_router.pt_bounces_dirty(settings.pt_max_bounces);
    let dispatch_plan =
        dirty_router.plan_dispatch(scene, draw_items_dirty, pt_bounces_dirty, 0, 0, 0);
    if dispatch_plan.has_work() {
        log::trace!(
            "dirty dispatch: cam={} lights={} inst={} pt_accum={}",
            dispatch_plan.camera_dirty,
            dispatch_plan.lights_dirty,
            dispatch_plan.instances_dirty,
            dispatch_plan.pt_accum_dirty,
        );
    }

    let extent = renderer.extent();
    renderer.set_taa(settings.taa, settings.taa_strength);
    renderer.set_anf_sr(settings.anf_sr, settings.anf_sharpness);
    renderer.set_mobile_upscaler(settings.mobile_upscaler, settings.upscaler_sharpness);
    let mut frame_data = FrameUBOData {
        view_proj: scene.view_proj.to_cols_array_2d(),
        camera_position: [
            scene.eye[0],
            scene.eye[1],
            scene.eye[2],
            scene.lights.len() as f32,
        ],
        light_direction: scene.light_direction.to_array(),
        light_color: scene.light_color.to_array(),
        view: scene.view.to_cols_array_2d(),
        light_view_proj: scene.light_view_proj.to_cols_array_2d(),
        light_view_proj1: scene.light_view_proj1.to_cols_array_2d(),
        shadow_split: scene.shadow_split,
        shadow_cascade_count: scene.shadow_cascade_count,
        tonemap_mode: settings.tonemap_mode,
        viewport_w: extent.width as f32,
        viewport_h: extent.height as f32,
        exposure: scene.exposure,
        shadow_strength: scene.shadow_strength,
        prev_view_proj: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        ..Default::default()
    };
    renderer.apply_taa_jitter(&mut frame_data);

    let mut resolved_grade = packet.grade.unwrap_or_else(|| settings.post_grade());
    resolved_grade.lut_strength = settings.lut_strength;
    let lut_path = if settings.lut.trim().is_empty() || prism_engine::asset_paths::strict_pak_only()
    {
        None
    } else {
        prism_engine::asset_paths::asset_path(settings.lut.trim())
    };
    renderer.set_internal_resolution(settings.internal_resolution, settings.internal_scale_mode);
    renderer.set_occlusion_culling(settings.occlusion_culling);

    {
        let renderer_settings = renderer.settings_mut();
        renderer_settings.auto_exposure = settings.auto_exposure;
        renderer_settings.raster_gi = settings.raster_gi;
        renderer_settings.pt_gi = settings.pt_gi;
        renderer_settings.prt_leak_bias = settings.prt_leak_bias;
        renderer_settings.sharc_capacity = settings.sharc_capacity;
        renderer_settings.sharc_scene_scale = settings.sharc_scene_scale;
        renderer_settings.sharc_enable_update = settings.sharc_enable_update;
        renderer_settings.sharc_enable_resolve = settings.sharc_enable_resolve;
        renderer_settings.sharc_enable_anti_firefly = settings.sharc_enable_anti_firefly;
        renderer_settings.sharc_roughness_threshold = settings.sharc_roughness_threshold;
        renderer_settings.sharc_downscale_factor = settings.sharc_downscale_factor;
        renderer_settings.sharc_accumulation_frame_num = settings.sharc_accumulation_frame_num;
        renderer_settings.sharc_stale_frame_num = settings.sharc_stale_frame_num;
        renderer_settings.restir_gi_scale = settings.restir_gi_scale;
        renderer_settings.restir_gi_rays_per_pixel = settings.restir_gi_rays_per_pixel;
        renderer_settings.restir_gi_spatial_samples = settings.restir_gi_spatial_samples;
        renderer_settings.restir_gi_max_history = settings.restir_gi_max_history;
        renderer_settings.restir_gi_temporal_reuse = settings.restir_gi_temporal_reuse;
        renderer_settings.restir_gi_spatial_reuse = settings.restir_gi_spatial_reuse;
        renderer_settings.restir_gi_max_secondary_dist = settings.restir_gi_max_secondary_dist;
        renderer_settings.dlss_mode = settings.dlss_mode;
        renderer_settings.dlss_pre_upscale = settings.dlss_pre_upscale;
        renderer_settings.dlfg_mode = settings.dlfg_mode;
        renderer_settings.dlfg_frames = settings.dlfg_frames.clamp(1, 3);
        renderer_settings.dlss_rr_enabled = settings.dlss_rr_enabled;
    }

    let begin_span = prism_engine::util::timing::span("render::begin_frame");
    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(false),
    };
    drop(begin_span);
    let mut occlusion_items = Vec::new();
    let main_pass_items: &[prism_render::render_graph::DrawItem] = if settings.occlusion_culling {
        occlusion_items.extend_from_slice(draw_items);
        renderer.cull_occluded_draws(&mut occlusion_items);
        &occlusion_items
    } else {
        draw_items
    };
    let input = FrameInput {
        draw_items: main_pass_items,
        gizmos: &packet.gizmos,
        sprite_items: &packet.sprite_items,
        sprite_lights: &packet.sprite_lights,
        particle_items: &packet.particle_items,
        bone_matrices: &packet.bone_matrices,
        skybox: packet.skybox,
        frame_data: &frame_data,
        cameras: &scene.cameras,
        light_view_proj: scene.light_view_proj.to_cols_array_2d(),
        light_view_proj1: scene.light_view_proj1.to_cols_array_2d(),
        shadow_split: scene.shadow_split,
        shadow_cascade_count: scene.shadow_cascade_count,
        inv_projection: scene.inv_projection.to_cols_array_2d(),
        debug_flags: settings.debug_flags,
        wireframe: settings.wireframe,
        shaded_wireframe: settings.shaded_wireframe,
        tonemap_mode: settings.tonemap_mode,
        aa_mode: settings.aa_mode,
        debug_rt: settings.debug_rt,
        grade: resolved_grade,
        lut_path: lut_path.as_deref(),
        proj22: scene.proj22,
        proj32: scene.proj32,
        lights: &scene.lights,
        render_mode: settings.render_mode,
        pt_max_bounces: settings.pt_max_bounces,
        pt_ray_max_distance: settings.pt_ray_max_distance,
        pt_max_iterations: settings.pt_max_iterations,
        exposure: scene.exposure,
        pt_lights: &scene.pt_lights,
        pt_accum_dirty: dispatch_plan.pt_accum_dirty,
        has_camera: scene.has_camera,
        camera_orthographic: scene.camera_orthographic,
        clear_color: scene.clear_color,
        shadows_enabled: scene.shadows_enabled,
        ui_overlay: Some(&packet.ui_overlay),
        show_grid: settings.show_grid,
        dispatch_plan: Some(&dispatch_plan),
    };
    if let Some(plan) = screenshot {
        renderer.request_screenshot(plan.path.clone(), plan.size, plan.scale);
    }
    renderer.set_voxel_scene(packet.voxel_scene.clone());
    let prepare_span = prism_engine::util::timing::span("render::prepare");
    let prepared = renderer.prepare(&ctx, &input);
    drop(prepare_span);
    let execute_span = prism_engine::util::timing::span("render::execute");
    let executed = prepared.and_then(|_| renderer.execute(&ctx, &input));
    drop(execute_span);
    executed.inspect_err(|_| {
        let _ = renderer.present(&ctx);
        let _ = renderer.end_frame(&ctx, false);
    })?;
    let present_span = prism_engine::util::timing::span("render::present");
    let out_of_date = renderer.present(&ctx)?;
    drop(present_span);
    if screenshot.is_some() {
        if let Err(err) = renderer.finish_screenshot(&ctx) {
            log::error!("screenshot failed: {err:#}");
        }
    }
    renderer.end_frame(&ctx, out_of_date)?;

    Ok(true)
}
