//! Self-contained `winit` + `egui-winit` + Ash Vulkan host replacing the external
//! `egui-ash` dependency.

mod renderer;

use std::ffi::CString;
use std::sync::{Arc, Mutex};

use ash::vk;
pub use raw_window_handle;
use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};
pub use renderer::ImageRegistry;
pub use winit;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use renderer::EguiRenderer;

pub mod event {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AppEvent {
        LoopExiting,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum Event<'a> {
        AppEvent { event: AppEvent },
        _Marker(std::marker::PhantomData<&'a ()>),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HandleRedraw {
    #[default]
    Auto,
}

#[derive(Clone, Default)]
pub struct ExitSignal {
    code: Arc<Mutex<Option<std::process::ExitCode>>>,
}

impl ExitSignal {
    pub fn send(&self, exit_code: std::process::ExitCode) {
        *self.code.lock().expect("exit signal poisoned") = Some(exit_code);
    }

    pub fn take_code(&self) -> Option<std::process::ExitCode> {
        *self.code.lock().expect("exit signal poisoned")
    }

    pub fn is_requested(&self) -> bool {
        self.code.lock().expect("exit signal poisoned").is_some()
    }
}

pub struct CreationContext<'a> {
    pub main_window: &'a Window,
    pub context: egui::Context,
    pub required_instance_extensions: Vec<CString>,
    pub required_device_extensions: Vec<CString>,
    pub image_registry: ImageRegistry,
    pub exit_signal: ExitSignal,
}

pub struct AshRenderState<A> {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub surface_loader: ash::khr::surface::Instance,
    pub swapchain_loader: ash::khr::swapchain::Device,
    pub queue: vk::Queue,
    #[allow(dead_code)]
    pub queue_family_index: u32,
    pub command_pool: vk::CommandPool,
    pub allocator: A,
}

#[derive(Default)]
pub struct RunOption {
    pub clear_color: [f32; 4],
    pub viewport_builder: Option<egui::ViewportBuilder>,
}

pub trait App {
    fn ui(&mut self, ctx: &egui::Context);

    fn handle_event(&mut self, _event: event::Event<'_>) {}

    fn request_redraw(&mut self, _viewport_id: egui::ViewportId) -> HandleRedraw {
        HandleRedraw::Auto
    }
}

pub trait AppCreator<A> {
    type App: App;

    fn create(&self, cc: CreationContext<'_>) -> (Self::App, AshRenderState<A>);
}

struct HostRunner<C> {
    title: String,
    options: RunOption,
    creator: Option<C>,
    window: Option<Window>,
    egui_state: Option<egui_winit::State>,
    egui_ctx: Option<egui::Context>,
    renderer: Option<EguiRenderer>,
    app: Option<Box<dyn App>>,
    exit_signal: ExitSignal,
    exiting_sent: bool,
}

impl<C> HostRunner<C>
where
    C: AppCreator<Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>>,
    C::App: 'static,
{
    fn notify_loop_exiting(&mut self) {
        if self.exiting_sent {
            return;
        }
        self.exiting_sent = true;
        if let Some(app) = self.app.as_mut() {
            app.handle_event(event::Event::AppEvent {
                event: event::AppEvent::LoopExiting,
            });
        }
    }
}

impl<C> ApplicationHandler for HostRunner<C>
where
    C: AppCreator<Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>>,
    C::App: 'static,
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = WindowAttributes::default().with_title(self.title.clone());
        if let Some(vb) = &self.options.viewport_builder {
            if let Some(title) = &vb.title {
                attrs = attrs.with_title(title.clone());
            }
            if let Some(size) = vb.inner_size {
                attrs = attrs.with_inner_size(LogicalSize::new(size.x as f64, size.y as f64));
            }
            if let Some(min_size) = vb.min_inner_size {
                attrs = attrs
                    .with_min_inner_size(LogicalSize::new(min_size.x as f64, min_size.y as f64));
            }
        }
        let window = event_loop
            .create_window(attrs)
            .expect("create editor window");

        let display_handle = window.display_handle().expect("window display handle");
        let window_handle = window.window_handle().expect("window raw handle");

        let ext_ptrs = ash_window::enumerate_required_extensions(display_handle.as_raw())
            .expect("enumerate required Vulkan surface extensions");
        let required_instance_extensions: Vec<CString> = ext_ptrs
            .iter()
            .map(|&p| unsafe { std::ffi::CStr::from_ptr(p) }.to_owned())
            .collect();
        let required_device_extensions = vec![ash::khr::swapchain::NAME.to_owned()];

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let registry = ImageRegistry::default();
        let cc = CreationContext {
            main_window: &window,
            context: egui_ctx.clone(),
            required_instance_extensions,
            required_device_extensions,
            image_registry: registry.clone(),
            exit_signal: self.exit_signal.clone(),
        };

        let creator = self.creator.take().expect("creator already consumed");
        let (app, render_state) = creator.create(cc);

        let surface = unsafe {
            ash_window::create_surface(
                &render_state.entry,
                &render_state.instance,
                display_handle.as_raw(),
                window_handle.as_raw(),
                None,
            )
        }
        .expect("create editor Vulkan surface");

        let size = window.inner_size();
        let renderer = EguiRenderer::new(
            &render_state.entry,
            &render_state.instance,
            render_state.physical_device,
            render_state.device,
            render_state.surface_loader,
            render_state.swapchain_loader,
            surface,
            render_state.queue,
            render_state.command_pool,
            render_state.allocator,
            registry,
            size.width.max(1),
            size.height.max(1),
            self.options.clear_color,
        )
        .expect("create editor egui Vulkan renderer");

        self.window = Some(window);
        self.egui_state = Some(egui_state);
        self.egui_ctx = Some(egui_ctx);
        self.renderer = Some(renderer);
        self.app = Some(Box::new(app));
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if let (Some(window), Some(state)) = (self.window.as_ref(), self.egui_state.as_mut()) {
            let response = state.on_window_event(window, &event);
            if response.repaint {
                window.request_redraw();
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.notify_loop_exiting();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Err(err) = renderer.resize(size.width, size.height) {
                        log::error!("editor swapchain resize failed: {err:#}");
                    }
                }
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if self.exit_signal.is_requested() {
                    self.notify_loop_exiting();
                    event_loop.exit();
                    return;
                }
                let (Some(window), Some(state), Some(ctx), Some(renderer), Some(app)) = (
                    self.window.as_ref(),
                    self.egui_state.as_mut(),
                    self.egui_ctx.as_ref(),
                    self.renderer.as_mut(),
                    self.app.as_mut(),
                ) else {
                    return;
                };

                let raw_input = state.take_egui_input(window);
                let full_output = ctx.run(raw_input, |ctx| {
                    app.ui(ctx);
                });
                state.handle_platform_output(window, full_output.platform_output);

                let pixels_per_point = full_output.pixels_per_point;
                let clipped_primitives =
                    ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
                let size = window.inner_size();
                if let Err(err) = renderer.paint(
                    ctx,
                    full_output.textures_delta,
                    clipped_primitives,
                    pixels_per_point,
                    size.width,
                    size.height,
                ) {
                    log::error!("editor frame paint failed: {err:#}");
                }

                if self.exit_signal.is_requested() {
                    self.notify_loop_exiting();
                    event_loop.exit();
                    return;
                }

                let _ = app.request_redraw(egui::ViewportId::ROOT);
                window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.exit_signal.is_requested() {
            self.notify_loop_exiting();
            event_loop.exit();
            return;
        }
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.notify_loop_exiting();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.destroy();
        }
    }
}

pub fn run<C>(title: &str, creator: C, options: RunOption) -> std::process::ExitCode
where
    C: AppCreator<Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>>,
    C::App: 'static,
{
    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(err) => {
            log::error!("failed to create winit event loop: {err}");
            return std::process::ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);

    let exit_signal = ExitSignal::default();
    let mut runner = HostRunner {
        title: title.to_string(),
        options,
        creator: Some(creator),
        window: None,
        egui_state: None,
        egui_ctx: None,
        renderer: None,
        app: None,
        exit_signal: exit_signal.clone(),
        exiting_sent: false,
    };
    if let Err(err) = event_loop.run_app(&mut runner) {
        log::error!("editor event loop error: {err}");
        return std::process::ExitCode::FAILURE;
    }
    exit_signal
        .take_code()
        .unwrap_or(std::process::ExitCode::SUCCESS)
}
