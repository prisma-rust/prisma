//! Surface materials: what a quad is made of, not how one widget looks.
//!
//! The model follows the desktop UI toolkits this editor is measured against:
//! every drawable surface carries an optional *material* (a shader plus its
//! parameters) and the host decides which pipeline to run for it. A surface
//! that samples the backdrop does so because its shader asks for the screen
//! texture, not because a widget set a blur flag.
//!
//! Consequences that matter for a self-hosted editor:
//!
//! * the editor ships two shaders (flat fill, frosted glass);
//! * a project registers its own by name and the kernel forwards it untouched;
//! * backdrop filtering is one such shader, so glass is not a special case;
//! * parameters are named, so a shader reads what it declared and ignores the
//!   rest without the kernel knowing any of its uniforms.

use crate::draw::UiTextureId;
use crate::style::Color;

/// How a texture is mapped onto a surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageMode {
    /// Stretch the texture to the surface.
    #[default]
    Stretch,
    /// Keep the corners while stretching the centre (9-slice), like a panel
    /// background that must not squash its rounded corners.
    NinePatch,
    /// Repeat the texture at its native size.
    Tile,
}

/// What a surface draws before its material is applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Surface {
    /// Nothing: the material alone decides the pixels.
    #[default]
    None,
    /// A flat colour fill.
    Solid(Color),
    /// A texture, optionally tinted.
    Image {
        texture: UiTextureId,
        tint: Color,
        mode: ImageMode,
        /// 9-slice insets in pixels: left, top, right, bottom.
        insets: [f32; 4],
    },
}

impl Surface {
    /// A tinted image stretched over the surface.
    pub fn image(texture: UiTextureId, tint: Color) -> Self {
        Self::Image {
            texture,
            tint,
            mode: ImageMode::Stretch,
            insets: [0.0; 4],
        }
    }

    /// A 9-slice image with explicit corner insets.
    pub fn nine_patch(texture: UiTextureId, tint: Color, insets: [f32; 4]) -> Self {
        Self::Image {
            texture,
            tint,
            mode: ImageMode::NinePatch,
            insets,
        }
    }

    /// The bindless texture this surface draws, if any.
    pub fn texture(&self) -> Option<UiTextureId> {
        match *self {
            Self::Image { texture, .. } => Some(texture),
            _ => None,
        }
    }

    /// The straight-alpha tint the surface multiplies into its output.
    pub fn tint(&self) -> Option<Color> {
        match *self {
            Self::None => None,
            Self::Solid(color) => Some(color),
            Self::Image { tint, .. } => Some(tint),
        }
    }
}

/// Largest backdrop blur radius the host is expected to honour, in pixels.
///
/// A material asking for more is clamped rather than rejected: filter cost
/// grows with the square of the radius, and a widget author picking 200 px
/// needs a comment far more than they need the extra taps.
pub const MAX_BACKDROP_SIGMA: f32 = 64.0;

/// One named shader parameter.
///
/// Values are the shapes a UI shader realistically declares. The kernel never
/// interprets a name; it forwards the block to the host, which matches names
/// against the shader it is about to bind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaterialValue {
    Scalar(f32),
    Vec2([f32; 2]),
    Vec4([f32; 4]),
    Color(Color),
}

impl MaterialValue {
    /// Pack the value into the float4 a host uploads per parameter.
    pub fn to_vec4(self) -> [f32; 4] {
        match self {
            Self::Scalar(v) => [v, 0.0, 0.0, 0.0],
            Self::Vec2(v) => [v[0], v[1], 0.0, 0.0],
            Self::Vec4(v) => v,
            Self::Color(c) => c.to_array(),
        }
    }

    /// The first scalar of the value, for parameters declared as a float.
    pub fn scalar(self) -> f32 {
        self.to_vec4()[0]
    }
}

impl From<f32> for MaterialValue {
    fn from(v: f32) -> Self {
        Self::Scalar(v)
    }
}

impl From<Color> for MaterialValue {
    fn from(v: Color) -> Self {
        Self::Color(v)
    }
}

impl From<[f32; 2]> for MaterialValue {
    fn from(v: [f32; 2]) -> Self {
        Self::Vec2(v)
    }
}

impl From<[f32; 4]> for MaterialValue {
    fn from(v: [f32; 4]) -> Self {
        Self::Vec4(v)
    }
}

/// Which shader the host must run to produce a surface.
///
/// Built-ins exist in every host. [`ShaderRef::Named`] is a host-registered
/// shader: the kernel forwards the name untouched, so a project can expose its
/// own surface shaders to editor chrome without the UI kernel learning any of
/// its details.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShaderRef {
    /// Flat fill plus an optional textured surface, the default.
    Standard,
    /// Frosted glass: filter the backdrop, then tint it.
    FrostedGlass,
    /// A shader the host registered under this name.
    Named(String),
}

impl ShaderRef {
    /// Stable name used in host registries and draw-command dumps.
    pub fn name(&self) -> &str {
        match self {
            Self::Standard => "standard",
            Self::FrostedGlass => "frosted_glass",
            Self::Named(name) => name,
        }
    }

    /// Whether the host has to publish a backdrop for this shader to sample.
    pub fn samples_backdrop(&self) -> bool {
        matches!(self, Self::FrostedGlass)
    }
}

impl Default for ShaderRef {
    fn default() -> Self {
        Self::Standard
    }
}

/// The material of one UI quad.
///
/// `None` means "host default": the surface draws as a flat tinted fill, which
/// is what every widget did before materials existed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Material {
    /// Shader the host runs for this surface.
    pub shader: ShaderRef,
    /// Named parameters matched against the shader's own declarations.
    pub params: Vec<(String, MaterialValue)>,
    /// Corner radius in pixels; the shader rounds the surface with it.
    pub radius: f32,
    /// Blur applied to the backdrop before compositing, in pixels.
    ///
    /// Zero means the host may skip its backdrop pass entirely.
    pub backdrop_sigma: f32,
}

impl Material {
    /// A flat fill with no parameters.
    pub fn solid() -> Self {
        Self::default()
    }

    /// Frosted glass: the host filters the backdrop and tints the result.
    pub fn frosted_glass(sigma: f32) -> Self {
        Self {
            shader: ShaderRef::FrostedGlass,
            params: Vec::new(),
            radius: 0.0,
            backdrop_sigma: sigma.clamp(0.0, MAX_BACKDROP_SIGMA),
        }
    }

    /// A surface rendered by a host-registered shader.
    pub fn from_shader(name: impl Into<String>) -> Self {
        Self {
            shader: ShaderRef::Named(name.into()),
            ..Self::default()
        }
    }

    /// Set one named parameter.
    pub fn with_param(mut self, name: impl Into<String>, value: impl Into<MaterialValue>) -> Self {
        let name = name.into();
        let value = value.into();
        match self.params.iter_mut().find(|(existing, _)| *existing == name) {
            Some(slot) => slot.1 = value,
            None => self.params.push((name, value)),
        }
        self
    }

    /// Read one named parameter back.
    pub fn param(&self, name: &str) -> Option<MaterialValue> {
        self.params
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, value)| *value)
    }

    /// Set the corner radius in pixels.
    pub fn with_radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Set the backdrop blur radius, clamped to what the host supports.
    pub fn with_backdrop_sigma(mut self, sigma: f32) -> Self {
        self.backdrop_sigma = sigma.clamp(0.0, MAX_BACKDROP_SIGMA);
        self
    }

    /// Whether the host must run its backdrop pass for this surface.
    pub fn needs_backdrop(&self) -> bool {
        self.backdrop_sigma > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frosted_glass_samples_the_backdrop() {
        let material = Material::frosted_glass(24.0);
        assert!(material.needs_backdrop());
        assert_eq!(material.shader, ShaderRef::FrostedGlass);
        assert!(material.shader.samples_backdrop());
        assert_eq!(material.backdrop_sigma, 24.0);
    }

    #[test]
    fn a_plain_material_never_asks_for_the_backdrop() {
        let material = Material::solid();
        assert!(!material.needs_backdrop());
        assert!(!material.shader.samples_backdrop());
    }

    #[test]
    fn the_blur_radius_is_clamped_not_rejected() {
        let material = Material::frosted_glass(500.0);
        assert_eq!(material.backdrop_sigma, MAX_BACKDROP_SIGMA);
    }

    #[test]
    fn a_named_shader_keeps_its_name_for_the_host() {
        let material = Material::from_shader("project/blur");
        assert_eq!(material.shader.name(), "project/blur");
        assert!(!material.needs_backdrop());
    }

    #[test]
    fn params_are_read_back_by_name_and_overwritten_in_place() {
        let material = Material::from_shader("project/glass")
            .with_param("edge_darkening", 0.35_f32)
            .with_param("edge_darkening", 0.5_f32)
            .with_param("highlight", Color::WHITE);
        assert_eq!(material.params.len(), 2);
        assert_eq!(
            material.param("edge_darkening").map(MaterialValue::scalar),
            Some(0.5)
        );
        assert_eq!(material.param("missing"), None);
    }

    #[test]
    fn a_surface_reports_its_texture_and_tint() {
        let surface = Surface::image(crate::draw::UiTextureId(7), Color::WHITE);
        assert_eq!(surface.texture(), Some(crate::draw::UiTextureId(7)));
        assert_eq!(surface.tint(), Some(Color::WHITE));
        assert_eq!(Surface::None.tint(), None);
    }
}
