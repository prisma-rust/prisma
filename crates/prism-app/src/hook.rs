//! Neutral "frame hook" --- `App` extension point, for debug host / future UI host to inject
//! arbitrary framework UI.
//!
//! `App` does not know any specific UI framework: hook handles its own window event handling and per-frame
//! UI running; render data via [`RenderShared::send_overlay_message`] with
//! type-erased messages fed to external overlay on render thread ([`SwapchainOverlay`]).

use prism_engine::ecs::World;
use prism_engine::render_settings::RenderSettings;
use prism_platform::{PlatformEvent, PlatformWindow};
use prism_render::SwapchainOverlay;

use crate::render_shared::{RenderShared, RenderStats};

/// Frame hook on main thread (debug / host UI etc).
///
/// Injection: `app(config).with_frame_hook(MyHook::new())`.
/// Hook all methods have default implementation---pure game host needs not implement any method.
///
/// `window` parameter is platform-layer neutral handle [`PlatformWindow`], **not** winit's
/// `Window`: thus when `winit-backend` off (Windows `win32-native`) hook
/// still gets window.
pub trait FrameHook: Send {
    /// Platform-agnostic event callback. New host should implement this method first.
    fn on_platform_event(&mut self, _window: &PlatformWindow, _event: PlatformEvent) -> bool {
        false
    }

    /// Return external overlay factory (optional).
    ///
    /// Called once before render thread starts: factory-produced [`SwapchainOverlay`] moved to
    /// render thread, record on top of ECS UI. GPU resources lazily created by implementation at record
    /// time---main thread only constructs pure CPU state.
    fn overlay(&self) -> Option<Box<dyn Fn() -> Box<dyn SwapchainOverlay> + Send>> {
        None
    }

    /// Called each frame (about_to_wait): run UI, read back render settings etc.
    fn on_tick(
        &mut self,
        _window: &PlatformWindow,
        _world: &mut World,
        _settings: &mut RenderSettings,
        _stats: &RenderStats,
        _shared: &RenderShared,
    ) {
    }

    /// winit raw window event forwarded (before app itself handles). Return true means event
    /// consumed, app no longer takes its own hotkey/input route (input still routes to InputManager
    /// to keep key state consistent).
    ///
    /// Only `winit-backend` build has this method; need cross-backend hook please implement
    /// [`Self::on_platform_event`].
    #[cfg(feature = "winit-backend")]
    fn on_window_event(
        &mut self,
        _window: &PlatformWindow,
        _event: &winit::event::WindowEvent,
    ) -> bool {
        false
    }
}
