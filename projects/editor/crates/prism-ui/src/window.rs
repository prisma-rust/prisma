//! Window descriptors and floating container windows.
//!
//! [`EditorWindowMeta`] describes a named tool window (title, minimum size,
//! closable flag), while [`ContainerWindow`] represents a floating host window
//! that owns a [`DockNode`](crate::dock::DockNode) root and participates in
//! Z-order focus, dragging, and 8-direction resizing.

use glam::Vec2;

use crate::dock::DockNode;
use crate::style::Rect;
use crate::tree::NodeKey;

/// Stable identifier of a logical editor window (e.g. `"hierarchy"`, `"inspector"`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub String);

impl WindowId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for WindowId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for WindowId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Unique identifier of a floating [`ContainerWindow`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContainerId(pub u64);

/// Presentation mode of a [`ContainerWindow`], mirroring Unity's `ShowMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShowMode {
    /// Normal floating tool window with title/tab bar and resize edges.
    #[default]
    Floating,
    /// Utility / inspector popup (typically frosted glass, stays above normal windows).
    Utility,
}

/// Static metadata registered for an editor window type/instance.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorWindowMeta {
    pub id: WindowId,
    pub title: String,
    pub min_size: Vec2,
    pub closable: bool,
    pub close_key: Option<NodeKey>,
}

impl EditorWindowMeta {
    pub fn new(id: impl Into<WindowId>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            min_size: Vec2::new(180.0, 120.0),
            closable: true,
            close_key: None,
        }
    }

    pub fn with_min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    pub fn with_closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    pub fn with_close_key(mut self, key: NodeKey) -> Self {
        self.close_key = Some(key);
        self
    }
}

/// Eight-way resize edge/corner of a floating [`ContainerWindow`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A floating host window containing a [`DockNode`] tree (typically a single
/// [`DockArea`](crate::dock::DockArea) or [`SplitView`](crate::dock::SplitView)).
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerWindow {
    pub id: ContainerId,
    pub rect: Rect,
    pub min_size: Vec2,
    pub mode: ShowMode,
    pub glass: bool,
    /// Optional blur sigma override; `None` uses [`Theme::glass_blur_sigma`](crate::theme::Theme::glass_blur_sigma).
    pub blur_sigma: Option<f32>,
    /// Optional tint alpha override; `None` uses [`Theme::glass_tint`](crate::theme::Theme::glass_tint).
    pub tint_alpha: Option<f32>,
    pub root: DockNode,
}

impl ContainerWindow {
    pub fn new(id: ContainerId, rect: Rect, root: DockNode) -> Self {
        Self {
            id,
            rect,
            min_size: Vec2::new(220.0, 160.0),
            mode: ShowMode::Floating,
            glass: true,
            blur_sigma: None,
            tint_alpha: None,
            root,
        }
    }

    pub fn with_mode(mut self, mode: ShowMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_glass(mut self, glass: bool) -> Self {
        self.glass = glass;
        self
    }

    pub fn with_blur_sigma(mut self, sigma: f32) -> Self {
        self.blur_sigma = Some(sigma);
        self
    }

    pub fn with_tint_alpha(mut self, alpha: f32) -> Self {
        self.tint_alpha = Some(alpha);
        self
    }

    pub fn with_min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    /// Clamp `self.rect` so the window stays within `bounds` and never shrinks
    /// below `self.min_size`.
    pub fn clamp_to_bounds(&mut self, bounds: Rect) {
        self.rect.w = self
            .rect
            .w
            .max(self.min_size.x)
            .min(bounds.w.max(self.min_size.x));
        self.rect.h = self
            .rect
            .h
            .max(self.min_size.y)
            .min(bounds.h.max(self.min_size.y));
        let max_x = (bounds.x + bounds.w - 48.0).max(bounds.x);
        let min_x = bounds.x - self.rect.w + 48.0;
        self.rect.x = self.rect.x.clamp(min_x, max_x);
        let max_y = (bounds.y + bounds.h - 28.0).max(bounds.y);
        self.rect.y = self.rect.y.clamp(bounds.y, max_y);
    }
}
