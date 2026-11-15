//! Extensible Unity-style Editor Gizmo API and PRT SH9 probe-volume visualization.
//!
//! Keep this module completely free of `egui` / Vulkan types so gizmo drawers
//! can be tested headlessly and reused across frontends. The viewport backend
//! consumes [`GizmoCommand`] values produced here and renders them onto the
//! Scene View painter.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::doc::{EntityJson, SceneDoc};
use crate::viewport_math as vm;

const COMPONENT_CAMERA: &str = "prism_engine::scene::Camera";
const COMPONENT_LIGHT: &str = "prism_engine::scene::Light";
const COMPONENT_DIRECTIONAL_LIGHT: &str = "prism_engine::scene::DirectionalLight";
const COMPONENT_POINT_LIGHT: &str = "prism_engine::scene::PointLight";
const COMPONENT_SPOT_LIGHT: &str = "prism_engine::scene::SpotLight";
const COMPONENT_COLLIDER: &str = "prism_engine::scene::Collider";
const COMPONENT_COLLIDER_2D: &str = "prism_engine::scene::Collider2D";
const COMPONENT_AUDIO_SOURCE: &str = "prism_engine::scene::AudioSource";
const COMPONENT_GI_CONFIG: &str = "prism_engine::scene::GiConfig";

// Ramamoorthi & Hanrahan order-2 real SH constants (must match `prism-render/src/gi.rs`).
const SH_C0: f32 = 0.282095;
const SH_C1: f32 = 0.488603;
const SH_C2: f32 = 1.092548;
const SH_C3: f32 = 0.315392;
const SH_C4: f32 = 0.546274;

const A_L0: f32 = PI;
const A_L1: f32 = 2.0 * PI / 3.0;
const A_L2: f32 = PI / 4.0;

const PRPV_MAGIC: &[u8; 4] = b"PRPV";
const PRPV_VERSION: u32 = 2;
const PRPV_FORMAT_F32: u32 = 0;
const PRPV_SCENE_NAME_LEN: usize = 64;
const PRPV_HEADER_SIZE: usize = 116;

/// Category used to filter groups of Scene View gizmos in the View menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GizmoCategory {
    Camera,
    Light,
    Collider,
    Audio,
    ProbeVolume,
    Custom(&'static str),
}

/// Active visibility filter passed when collecting Scene View gizmos.
#[derive(Clone, Debug)]
pub struct GizmoFilter<'a> {
    pub show_cameras: bool,
    pub show_lights: bool,
    pub show_colliders: bool,
    pub show_audio: bool,
    pub prt: &'a PrtGizmoSettings,
}

impl<'a> GizmoFilter<'a> {
    /// Return whether gizmos of `category` are currently enabled.
    pub fn is_visible(&self, category: GizmoCategory, selected: bool) -> bool {
        match category {
            GizmoCategory::Camera => self.show_cameras,
            GizmoCategory::Light => self.show_lights,
            GizmoCategory::Collider => self.show_colliders || selected,
            GizmoCategory::Audio => self.show_audio || selected,
            GizmoCategory::ProbeVolume => self.prt.enabled || selected,
            GizmoCategory::Custom(_) => true,
        }
    }
}

/// Logical icon badge identifier rendered on top of an entity's world anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoBadgeIcon {
    Camera,
    DirectionalLight,
    PointLight,
    SpotLight,
    AudioSource,
    ProbeVolume,
    Custom(&'static str),
}

impl GizmoBadgeIcon {
    /// Short glyph label drawn inside the Scene View badge chip.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Camera => "CAM",
            Self::DirectionalLight => "SUN",
            Self::PointLight => "PT",
            Self::SpotLight => "SPT",
            Self::AudioSource => "SND",
            Self::ProbeVolume => "GI",
            Self::Custom(tag) => tag,
        }
    }
}

/// Frontend-agnostic drawing primitive emitted by gizmo drawers.
#[derive(Clone, Debug, PartialEq)]
pub enum GizmoCommand {
    Line {
        start: [f32; 3],
        end: [f32; 3],
        color: [u8; 4],
        width: f32,
    },
    WireBox {
        center: [f32; 3],
        half_extents: [f32; 3],
        rotation: [f32; 4],
        color: [u8; 4],
        width: f32,
    },
    WireSphere {
        center: [f32; 3],
        radius: f32,
        rotation: [f32; 4],
        color: [u8; 4],
        width: f32,
    },
    WireCapsule {
        center: [f32; 3],
        radius: f32,
        half_height: f32,
        axis: u8,
        rotation: [f32; 4],
        color: [u8; 4],
        width: f32,
    },
    WireCone {
        apex: [f32; 3],
        direction: [f32; 3],
        length: f32,
        half_angle_rad: f32,
        basis_x: [f32; 3],
        basis_y: [f32; 3],
        color: [u8; 4],
        width: f32,
    },
    WireFrustum {
        origin: [f32; 3],
        basis: [[f32; 3]; 3],
        near: f32,
        far: f32,
        near_half: [f32; 2],
        far_half: [f32; 2],
        color: [u8; 4],
        width: f32,
    },
    ScreenBadge {
        world_pos: [f32; 3],
        icon: GizmoBadgeIcon,
        bg: [u8; 4],
        border: [u8; 4],
        fg: [u8; 4],
        pickable: bool,
        entity_index: Option<usize>,
    },
    SolidSphere {
        center: [f32; 3],
        radius: f32,
        fill: [u8; 4],
        outline: Option<[u8; 4]>,
    },
    ShProbeSphere {
        center: [f32; 3],
        radius: f32,
        sh: [[f32; 3]; 9],
        exposure: f32,
        outline: Option<[u8; 4]>,
    },
    WorldLabel {
        world_pos: [f32; 3],
        text: String,
        color: [u8; 4],
    },
}

/// Immediate-mode gizmo collector passed to [`ComponentGizmoDrawer::draw`].
#[derive(Default, Debug)]
pub struct GizmoContext {
    commands: Vec<GizmoCommand>,
}

impl GizmoContext {
    /// Create an empty gizmo command buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Access all collected commands in submission order.
    pub fn commands(&self) -> &[GizmoCommand] {
        &self.commands
    }

    /// Consume the context and return all collected commands.
    pub fn into_commands(self) -> Vec<GizmoCommand> {
        self.commands
    }

    /// Push a world-space line segment.
    pub fn draw_line(&mut self, start: [f32; 3], end: [f32; 3], color: [u8; 4], width: f32) {
        self.commands.push(GizmoCommand::Line {
            start,
            end,
            color,
            width,
        });
    }

    /// Push an oriented wireframe box.
    pub fn draw_wire_box(
        &mut self,
        center: [f32; 3],
        half_extents: [f32; 3],
        rotation: [f32; 4],
        color: [u8; 4],
        width: f32,
    ) {
        self.commands.push(GizmoCommand::WireBox {
            center,
            half_extents,
            rotation,
            color,
            width,
        });
    }

    /// Push a 3-ring wireframe sphere.
    pub fn draw_wire_sphere(
        &mut self,
        center: [f32; 3],
        radius: f32,
        rotation: [f32; 4],
        color: [u8; 4],
        width: f32,
    ) {
        self.commands.push(GizmoCommand::WireSphere {
            center,
            radius: radius.max(0.001),
            rotation,
            color,
            width,
        });
    }

    /// Push a wireframe capsule oriented along local `axis` (`0=X`, `1=Y`, `2=Z`).
    pub fn draw_wire_capsule(
        &mut self,
        center: [f32; 3],
        radius: f32,
        half_height: f32,
        axis: u8,
        rotation: [f32; 4],
        color: [u8; 4],
        width: f32,
    ) {
        self.commands.push(GizmoCommand::WireCapsule {
            center,
            radius: radius.max(0.001),
            half_height: half_height.max(0.0),
            axis: axis.min(2),
            rotation,
            color,
            width,
        });
    }

    /// Push a wireframe spotlight cone.
    pub fn draw_wire_cone(
        &mut self,
        apex: [f32; 3],
        direction: [f32; 3],
        length: f32,
        half_angle_rad: f32,
        basis_x: [f32; 3],
        basis_y: [f32; 3],
        color: [u8; 4],
        width: f32,
    ) {
        self.commands.push(GizmoCommand::WireCone {
            apex,
            direction,
            length: length.max(0.01),
            half_angle_rad: half_angle_rad.clamp(0.01, PI * 0.49),
            basis_x,
            basis_y,
            color,
            width,
        });
    }

    /// Push a wireframe camera frustum.
    pub fn draw_wire_frustum(
        &mut self,
        origin: [f32; 3],
        basis: [[f32; 3]; 3],
        near: f32,
        far: f32,
        near_half: [f32; 2],
        far_half: [f32; 2],
        color: [u8; 4],
        width: f32,
    ) {
        self.commands.push(GizmoCommand::WireFrustum {
            origin,
            basis,
            near,
            far,
            near_half,
            far_half,
            color,
            width,
        });
    }

    /// Push a clickable Scene View icon badge at `world_pos`.
    pub fn draw_screen_badge(
        &mut self,
        world_pos: [f32; 3],
        icon: GizmoBadgeIcon,
        bg: [u8; 4],
        border: [u8; 4],
        fg: [u8; 4],
        pickable: bool,
        entity_index: Option<usize>,
    ) {
        self.commands.push(GizmoCommand::ScreenBadge {
            world_pos,
            icon,
            bg,
            border,
            fg,
            pickable,
            entity_index,
        });
    }

    /// Push a flat-colored screen-projected sphere (e.g., mean irradiance mode).
    pub fn draw_solid_sphere(
        &mut self,
        center: [f32; 3],
        radius: f32,
        fill: [u8; 4],
        outline: Option<[u8; 4]>,
    ) {
        self.commands.push(GizmoCommand::SolidSphere {
            center,
            radius: radius.max(0.01),
            fill,
            outline,
        });
    }

    /// Push a 2nd-order spherical-harmonic shaded PRT probe sphere.
    pub fn draw_sh_probe_sphere(
        &mut self,
        center: [f32; 3],
        radius: f32,
        sh: [[f32; 3]; 9],
        exposure: f32,
        outline: Option<[u8; 4]>,
    ) {
        self.commands.push(GizmoCommand::ShProbeSphere {
            center,
            radius: radius.max(0.01),
            sh,
            exposure,
            outline,
        });
    }

    /// Push a small screen label anchored at `world_pos`.
    pub fn draw_world_label(
        &mut self,
        world_pos: [f32; 3],
        text: impl Into<String>,
        color: [u8; 4],
    ) {
        self.commands.push(GizmoCommand::WorldLabel {
            world_pos,
            text: text.into(),
            color,
        });
    }
}

/// Entity context handed to a [`ComponentGizmoDrawer`].
pub struct GizmoEntityContext<'a> {
    pub doc: &'a SceneDoc,
    pub entity_index: usize,
    pub entity: &'a EntityJson,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub basis: [[f32; 3]; 3],
    /// Engine forward (`-Z` in local space).
    pub forward: [f32; 3],
    pub selected: bool,
    pub viewport_aspect: f32,
    pub camera_world_len: f32,
}

impl<'a> GizmoEntityContext<'a> {
    /// Read a floating-point field from `component` or return `None`.
    pub fn component_f32(&self, component: &str, field: &str) -> Option<f32> {
        self.entity
            .component(component)?
            .get(field)?
            .as_f64()
            .map(|v| v as f32)
    }

    /// Read a boolean field from `component`, defaulting to `false`.
    pub fn component_bool(&self, component: &str, field: &str) -> bool {
        self.entity
            .component(component)
            .and_then(|c| c.get(field))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Read a `[f32; 3]` array field from `component`.
    pub fn component_vec3(&self, component: &str, field: &str) -> Option<[f32; 3]> {
        let arr = self.entity.component(component)?.get(field)?;
        Some([
            arr.get(0)?.as_f64()? as f32,
            arr.get(1)?.as_f64()? as f32,
            arr.get(2)?.as_f64()? as f32,
        ])
    }

    /// Read a `[f32; 2]` array field from `component`.
    pub fn component_vec2(&self, component: &str, field: &str) -> Option<[f32; 2]> {
        let arr = self.entity.component(component)?.get(field)?;
        Some([arr.get(0)?.as_f64()? as f32, arr.get(1)?.as_f64()? as f32])
    }

    /// Transform a local offset vector into world space (scaled + rotated + translated).
    pub fn local_point_to_world(&self, local: [f32; 3]) -> [f32; 3] {
        let q = Quat::from_xyzw(
            self.rotation[0],
            self.rotation[1],
            self.rotation[2],
            self.rotation[3],
        )
        .normalize();
        let scaled = Vec3::new(
            local[0] * self.scale[0],
            local[1] * self.scale[1],
            local[2] * self.scale[2],
        );
        let p = Vec3::from_array(self.position) + q * scaled;
        p.to_array()
    }

    fn base_alpha(&self) -> u8 {
        if self.selected {
            255
        } else {
            190
        }
    }

    fn stroke_width(&self) -> f32 {
        if self.selected {
            1.6
        } else {
            1.1
        }
    }
}

/// Unity-equivalent per-component gizmo drawer trait.
///
/// Custom components or editor plugins can register their own drawers on
/// [`GizmoRegistry`] to render world-space debug geometry and pickable badges.
pub trait ComponentGizmoDrawer: Send + Sync {
    /// Full reflected component type name (e.g. `"prism_engine::scene::Camera"`).
    fn component_name(&self) -> &'static str;

    /// Category toggle controlling whether this drawer runs when unselected.
    fn category(&self) -> GizmoCategory;

    /// Emit zero or more gizmo primitives into `gizmos`.
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext);
}

/// Adapter turning a closure into a [`ComponentGizmoDrawer`].
struct FnGizmoDrawer<F> {
    component_name: &'static str,
    category: GizmoCategory,
    draw_fn: F,
}

impl<F> ComponentGizmoDrawer for FnGizmoDrawer<F>
where
    F: Fn(&GizmoEntityContext<'_>, &mut GizmoContext) + Send + Sync,
{
    fn component_name(&self) -> &'static str {
        self.component_name
    }

    fn category(&self) -> GizmoCategory {
        self.category
    }

    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        (self.draw_fn)(ctx, gizmos);
    }
}

/// Registry mapping component type names to [`ComponentGizmoDrawer`] plugins.
pub struct GizmoRegistry {
    drawers: HashMap<&'static str, Vec<Box<dyn ComponentGizmoDrawer>>>,
}

impl Default for GizmoRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl GizmoRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            drawers: HashMap::new(),
        }
    }

    /// Create a registry pre-populated with all built-in Prisma scene drawers.
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();
        reg.register(CameraGizmoDrawer);
        reg.register(DirectionalLightGizmoDrawer);
        reg.register(PointLightGizmoDrawer);
        reg.register(SpotLightGizmoDrawer);
        reg.register(LegacyLightGizmoDrawer);
        reg.register(ColliderGizmoDrawer);
        reg.register(Collider2DGizmoDrawer);
        reg.register(AudioSourceGizmoDrawer);
        reg.register(GiVolumeGizmoDrawer);
        reg
    }

    /// Register a [`ComponentGizmoDrawer`] instance for its component type.
    pub fn register<D: ComponentGizmoDrawer + 'static>(&mut self, drawer: D) {
        self.drawers
            .entry(drawer.component_name())
            .or_default()
            .push(Box::new(drawer));
    }

    /// Convenience helper for registering a closure-based component gizmo drawer.
    pub fn register_fn<F>(&mut self, component_name: &'static str, category: GizmoCategory, f: F)
    where
        F: Fn(&GizmoEntityContext<'_>, &mut GizmoContext) + Send + Sync + 'static,
    {
        self.register(FnGizmoDrawer {
            component_name,
            category,
            draw_fn: f,
        });
    }

    /// Number of distinct component types with registered drawers.
    pub fn len(&self) -> usize {
        self.drawers.len()
    }

    /// Whether the registry has zero drawers.
    pub fn is_empty(&self) -> bool {
        self.drawers.is_empty()
    }

    /// Collect gizmos for all visible entities in `doc` matching `filter`.
    pub fn collect_scene(
        &self,
        doc: &SceneDoc,
        selection: &[usize],
        hidden_entities: &[u32],
        isolate_visible: Option<&[bool]>,
        viewport_aspect: f32,
        camera_world_len: f32,
        filter: &GizmoFilter<'_>,
        out: &mut GizmoContext,
    ) {
        let aspect = if viewport_aspect.is_finite() && viewport_aspect > 0.01 {
            viewport_aspect
        } else {
            16.0 / 9.0
        };

        for (index, entity) in doc.entities.iter().enumerate() {
            if !entity.active() {
                continue;
            }
            let entity_id = doc.id_of_index(index).unwrap_or(0);
            if hidden_entities.contains(&entity_id) {
                continue;
            }
            if let Some(mask) = isolate_visible {
                if !mask.get(index).copied().unwrap_or(false) {
                    continue;
                }
            }
            let selected = selection.contains(&index);
            let position = vm::world_position(doc, index);
            let rotation = vm::world_rotation(doc, index);
            let scale = vm::world_scale(doc, index);
            let basis = vm::basis_from_quat(rotation);
            let forward = [-basis[2][0], -basis[2][1], -basis[2][2]];

            let entity_ctx = GizmoEntityContext {
                doc,
                entity_index: index,
                entity,
                position,
                rotation,
                scale,
                basis,
                forward,
                selected,
                viewport_aspect: aspect,
                camera_world_len,
            };

            for comp_name in entity.components.keys() {
                let Some(list) = self.drawers.get(comp_name.as_str()) else {
                    continue;
                };
                for drawer in list {
                    if filter.is_visible(drawer.category(), selected) {
                        drawer.draw(&entity_ctx, out);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in component gizmo drawers
// ---------------------------------------------------------------------------

fn component_rgb_with_alpha(entity: &EntityJson, component: &str, alpha: u8) -> [u8; 4] {
    let default = [255, 224, 150, alpha];
    let Some(color) = entity.component(component).and_then(|c| c.get("color")) else {
        return default;
    };
    let Some(rgb) = color.as_array() else {
        return default;
    };
    if rgb.len() < 3 {
        return default;
    }
    let ch = |idx: usize| -> u8 {
        let v = rgb[idx].as_f64().unwrap_or(1.0) as f32;
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [ch(0), ch(1), ch(2), alpha]
}

struct CameraGizmoDrawer;

impl ComponentGizmoDrawer for CameraGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_CAMERA
    }

    fn category(&self) -> GizmoCategory {
        GizmoCategory::Camera
    }

    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        let alpha = ctx.base_alpha();
        let color = if ctx.selected {
            [170, 212, 255, alpha]
        } else {
            [140, 178, 222, alpha]
        };
        gizmos.draw_screen_badge(
            ctx.position,
            GizmoBadgeIcon::Camera,
            [24, 30, 42, alpha],
            color,
            [225, 236, 250, alpha],
            true,
            Some(ctx.entity_index),
        );

        let orthographic = ctx.component_bool(COMPONENT_CAMERA, "orthographic");
        let near = ctx
            .component_f32(COMPONENT_CAMERA, "near")
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(0.1);
        let far = ctx
            .component_f32(COMPONENT_CAMERA, "far")
            .filter(|v| v.is_finite() && *v > near)
            .unwrap_or(1000.0);
        let (near_hw, near_hh, far_hw, far_hh) = if orthographic {
            let half_h = ctx
                .component_f32(COMPONENT_CAMERA, "orthographic_size")
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(5.0);
            let half_w = half_h * ctx.viewport_aspect;
            (half_w, half_h, half_w, half_h)
        } else {
            let fov = ctx
                .component_f32(COMPONENT_CAMERA, "fov_y_degrees")
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(60.0);
            let half_tan = (fov.to_radians() * 0.5).tan();
            (
                half_tan * near * ctx.viewport_aspect,
                half_tan * near,
                half_tan * far * ctx.viewport_aspect,
                half_tan * far,
            )
        };
        gizmos.draw_wire_frustum(
            ctx.position,
            ctx.basis,
            near,
            far,
            [near_hw, near_hh],
            [far_hw, far_hh],
            color,
            ctx.stroke_width(),
        );
    }
}

fn draw_directional_light_gizmo(
    ctx: &GizmoEntityContext<'_>,
    component: &str,
    gizmos: &mut GizmoContext,
) {
    let alpha = ctx.base_alpha();
    let color = component_rgb_with_alpha(ctx.entity, component, alpha);
    gizmos.draw_screen_badge(
        ctx.position,
        GizmoBadgeIcon::DirectionalLight,
        [38, 34, 22, alpha],
        color,
        color,
        true,
        Some(ctx.entity_index),
    );
    let len = ctx.camera_world_len.max(0.8) * 1.25;
    let tip = [
        ctx.position[0] + ctx.forward[0] * len,
        ctx.position[1] + ctx.forward[1] * len,
        ctx.position[2] + ctx.forward[2] * len,
    ];
    gizmos.draw_line(ctx.position, tip, color, ctx.stroke_width());
    if ctx.selected {
        let r = len * 0.28;
        for (du, dv) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let offset = [
                ctx.basis[0][0] * r * du + ctx.basis[1][0] * r * dv,
                ctx.basis[0][1] * r * du + ctx.basis[1][1] * r * dv,
                ctx.basis[0][2] * r * du + ctx.basis[1][2] * r * dv,
            ];
            let s = [
                ctx.position[0] + offset[0],
                ctx.position[1] + offset[1],
                ctx.position[2] + offset[2],
            ];
            let e = [tip[0] + offset[0], tip[1] + offset[1], tip[2] + offset[2]];
            gizmos.draw_line(s, e, [color[0], color[1], color[2], 140], 1.0);
        }
    }
}

fn draw_point_light_gizmo(
    ctx: &GizmoEntityContext<'_>,
    component: &str,
    gizmos: &mut GizmoContext,
) {
    let alpha = ctx.base_alpha();
    let color = component_rgb_with_alpha(ctx.entity, component, alpha);
    gizmos.draw_screen_badge(
        ctx.position,
        GizmoBadgeIcon::PointLight,
        [36, 32, 22, alpha],
        color,
        color,
        true,
        Some(ctx.entity_index),
    );
    if ctx.selected {
        let range = ctx
            .component_f32(component, "range")
            .filter(|v| v.is_finite() && *v > 0.05)
            .unwrap_or(10.0);
        gizmos.draw_wire_sphere(
            ctx.position,
            range,
            [0.0, 0.0, 0.0, 1.0],
            [color[0], color[1], color[2], 150],
            1.2,
        );
    }
}

fn draw_spot_light_gizmo(ctx: &GizmoEntityContext<'_>, component: &str, gizmos: &mut GizmoContext) {
    let alpha = ctx.base_alpha();
    let color = component_rgb_with_alpha(ctx.entity, component, alpha);
    gizmos.draw_screen_badge(
        ctx.position,
        GizmoBadgeIcon::SpotLight,
        [36, 32, 22, alpha],
        color,
        color,
        true,
        Some(ctx.entity_index),
    );
    let outer_half = ctx
        .component_f32(component, "spot_angle")
        .map(|deg| deg.to_radians() * 0.5)
        .or_else(|| ctx.component_f32(component, "outer_cone_angle"))
        .unwrap_or(0.52);
    let len = if ctx.selected {
        ctx.component_f32(component, "range")
            .filter(|v| v.is_finite() && *v > 0.1)
            .unwrap_or(ctx.camera_world_len * 1.4)
    } else {
        ctx.camera_world_len * 1.2
    };
    gizmos.draw_wire_cone(
        ctx.position,
        ctx.forward,
        len,
        outer_half,
        ctx.basis[0],
        ctx.basis[1],
        color,
        ctx.stroke_width(),
    );
    if ctx.selected {
        if let Some(inner_half) = ctx.component_f32(component, "inner_cone_angle") {
            if inner_half > 0.02 && inner_half < outer_half - 0.02 {
                gizmos.draw_wire_cone(
                    ctx.position,
                    ctx.forward,
                    len,
                    inner_half,
                    ctx.basis[0],
                    ctx.basis[1],
                    [color[0], color[1], color[2], 125],
                    1.0,
                );
            }
        }
    }
}

struct DirectionalLightGizmoDrawer;
impl ComponentGizmoDrawer for DirectionalLightGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_DIRECTIONAL_LIGHT
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::Light
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        draw_directional_light_gizmo(ctx, COMPONENT_DIRECTIONAL_LIGHT, gizmos);
    }
}

struct PointLightGizmoDrawer;
impl ComponentGizmoDrawer for PointLightGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_POINT_LIGHT
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::Light
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        draw_point_light_gizmo(ctx, COMPONENT_POINT_LIGHT, gizmos);
    }
}

struct SpotLightGizmoDrawer;
impl ComponentGizmoDrawer for SpotLightGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_SPOT_LIGHT
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::Light
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        draw_spot_light_gizmo(ctx, COMPONENT_SPOT_LIGHT, gizmos);
    }
}

struct LegacyLightGizmoDrawer;
impl ComponentGizmoDrawer for LegacyLightGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_LIGHT
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::Light
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        let kind = ctx
            .entity
            .component(COMPONENT_LIGHT)
            .and_then(|v| v.get("light_type"))
            .and_then(Value::as_str)
            .unwrap_or("Point");
        match kind {
            "Directional" | "Sun" => draw_directional_light_gizmo(ctx, COMPONENT_LIGHT, gizmos),
            "Spot" => draw_spot_light_gizmo(ctx, COMPONENT_LIGHT, gizmos),
            _ => draw_point_light_gizmo(ctx, COMPONENT_LIGHT, gizmos),
        }
    }
}

struct ColliderGizmoDrawer;
impl ComponentGizmoDrawer for ColliderGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_COLLIDER
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::Collider
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        let center_local = ctx
            .component_vec3(COMPONENT_COLLIDER, "center")
            .unwrap_or([0.0, 0.0, 0.0]);
        let world_center = ctx.local_point_to_world(center_local);
        let is_trigger = ctx.component_bool(COMPONENT_COLLIDER, "is_trigger");
        let alpha = if ctx.selected { 235 } else { 145 };
        let color = if is_trigger {
            [115, 218, 175, alpha]
        } else {
            [98, 216, 118, alpha]
        };
        let width = if ctx.selected { 1.5 } else { 1.0 };
        let max_scale = ctx.scale[0]
            .abs()
            .max(ctx.scale[1].abs())
            .max(ctx.scale[2].abs())
            .max(0.001);

        let shape = ctx
            .entity
            .component(COMPONENT_COLLIDER)
            .and_then(|c| c.get("shape"))
            .and_then(Value::as_str)
            .unwrap_or("Box");
        match shape {
            "Sphere" => {
                let r = ctx
                    .component_f32(COMPONENT_COLLIDER, "radius")
                    .unwrap_or(0.5)
                    .max(0.001)
                    * max_scale;
                gizmos.draw_wire_sphere(world_center, r, ctx.rotation, color, width);
            }
            "Capsule" => {
                let r = ctx
                    .component_f32(COMPONENT_COLLIDER, "radius")
                    .unwrap_or(0.5)
                    .max(0.001)
                    * ctx.scale[0].abs().max(ctx.scale[2].abs()).max(0.001);
                let hh = ctx
                    .component_f32(COMPONENT_COLLIDER, "half_height")
                    .unwrap_or(0.5)
                    .max(0.0)
                    * ctx.scale[1].abs().max(0.001);
                gizmos.draw_wire_capsule(world_center, r, hh, 1, ctx.rotation, color, width);
            }
            _ => {
                let half = ctx
                    .component_vec3(COMPONENT_COLLIDER, "half_extents")
                    .unwrap_or([0.5, 0.5, 0.5]);
                let scaled_half = [
                    half[0].abs().max(0.001) * ctx.scale[0].abs().max(0.001),
                    half[1].abs().max(0.001) * ctx.scale[1].abs().max(0.001),
                    half[2].abs().max(0.001) * ctx.scale[2].abs().max(0.001),
                ];
                gizmos.draw_wire_box(world_center, scaled_half, ctx.rotation, color, width);
            }
        }
    }
}

struct Collider2DGizmoDrawer;
impl ComponentGizmoDrawer for Collider2DGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_COLLIDER_2D
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::Collider
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        let offset = ctx
            .component_vec2(COMPONENT_COLLIDER_2D, "offset")
            .unwrap_or([0.0, 0.0]);
        let half = ctx
            .component_vec2(COMPONENT_COLLIDER_2D, "half_extents")
            .unwrap_or([0.5, 0.5]);
        let world_center = ctx.local_point_to_world([offset[0], offset[1], 0.0]);
        let alpha = if ctx.selected { 235 } else { 145 };
        let width = if ctx.selected { 1.5 } else { 1.0 };
        gizmos.draw_wire_box(
            world_center,
            [
                half[0].abs().max(0.001) * ctx.scale[0].abs().max(0.001),
                half[1].abs().max(0.001) * ctx.scale[1].abs().max(0.001),
                0.02,
            ],
            ctx.rotation,
            [98, 216, 118, alpha],
            width,
        );
    }
}

struct AudioSourceGizmoDrawer;
impl ComponentGizmoDrawer for AudioSourceGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_AUDIO_SOURCE
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::Audio
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        let alpha = ctx.base_alpha();
        let border = [120, 198, 240, alpha];
        gizmos.draw_screen_badge(
            ctx.position,
            GizmoBadgeIcon::AudioSource,
            [22, 34, 42, alpha],
            border,
            [180, 230, 255, alpha],
            true,
            Some(ctx.entity_index),
        );
        if ctx.selected {
            let min_d = ctx
                .component_f32(COMPONENT_AUDIO_SOURCE, "min_distance")
                .unwrap_or(1.0)
                .max(0.05);
            let max_d = ctx
                .component_f32(COMPONENT_AUDIO_SOURCE, "max_distance")
                .unwrap_or(50.0)
                .max(min_d);
            gizmos.draw_wire_sphere(
                ctx.position,
                min_d,
                [0.0, 0.0, 0.0, 1.0],
                [120, 210, 250, 200],
                1.3,
            );
            gizmos.draw_wire_sphere(
                ctx.position,
                max_d,
                [0.0, 0.0, 0.0, 1.0],
                [120, 210, 250, 95],
                1.0,
            );
        }
    }
}

struct GiVolumeGizmoDrawer;
impl ComponentGizmoDrawer for GiVolumeGizmoDrawer {
    fn component_name(&self) -> &'static str {
        COMPONENT_GI_CONFIG
    }
    fn category(&self) -> GizmoCategory {
        GizmoCategory::ProbeVolume
    }
    fn draw(&self, ctx: &GizmoEntityContext<'_>, gizmos: &mut GizmoContext) {
        let enabled = ctx.component_bool(COMPONENT_GI_CONFIG, "enabled");
        let alpha = ctx.base_alpha();
        let border = if enabled {
            [88, 208, 224, alpha]
        } else {
            [130, 140, 150, alpha]
        };
        gizmos.draw_screen_badge(
            ctx.position,
            GizmoBadgeIcon::ProbeVolume,
            [22, 36, 40, alpha],
            border,
            [190, 242, 250, alpha],
            true,
            Some(ctx.entity_index),
        );
    }
}

// ---------------------------------------------------------------------------
// PRT Probe Volume (PRPV) parsing, SH9 evaluation & Scene View visualization
// ---------------------------------------------------------------------------

/// Visualization style for PRT probe volume gizmos in Scene View.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PrtDisplayMode {
    /// Full 2nd-order spherical harmonics shaded micro-spheres.
    #[default]
    ShSpheres,
    /// Flat spheres shaded by DC mean irradiance.
    MeanIrradiance,
    /// Dominant incoming light direction needles paired with DC probe spheres.
    DominantDirection,
    /// Only draw the probe volume world AABB cage and slice frame.
    GridBoundsOnly,
}

impl PrtDisplayMode {
    pub const ALL: [Self; 4] = [
        Self::ShSpheres,
        Self::MeanIrradiance,
        Self::DominantDirection,
        Self::GridBoundsOnly,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ShSpheres => "SH9 Spheres",
            Self::MeanIrradiance => "Mean Irradiance",
            Self::DominantDirection => "Dominant Direction",
            Self::GridBoundsOnly => "Bounds Only",
        }
    }
}

/// User-configurable PRT probe visualization settings stored in [`crate::Workspace`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrtGizmoSettings {
    pub enabled: bool,
    pub mode: PrtDisplayMode,
    /// Probe sphere radius in world units (clamped dynamically to fit screen nicely).
    pub probe_radius: f32,
    /// Tone-mapping exposure multiplier applied to SH irradiance reconstruction.
    pub exposure: f32,
    /// Maximum probes rendered at once (auto-strides larger grids evenly).
    pub max_probes: usize,
    pub show_grid_bounds: bool,
    pub show_trilinear_tethers: bool,
    /// Optional horizontal Y-layer filter (`None` = all layers).
    pub slice_y: Option<u32>,
}

impl Default for PrtGizmoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: PrtDisplayMode::ShSpheres,
            probe_radius: 0.22,
            exposure: 1.0,
            max_probes: 320,
            show_grid_bounds: true,
            show_trilinear_tethers: true,
            slice_y: None,
        }
    }
}

/// CPU representation of a `PRPV` v2 baked GI probe volume for editor visualization.
#[derive(Clone, Debug, PartialEq)]
pub struct PrtProbeVolume {
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub dims: [u32; 3],
    /// `dims[0] * dims[1] * dims[2] * 9` RGB SH coefficients.
    pub coeffs: Vec<[f32; 3]>,
    pub scene_name: String,
    pub global_hit_ratio: f32,
}

/// Lightweight header metadata without loading full SH coefficients into memory.
#[derive(Clone, Debug, PartialEq)]
pub struct PrtHeaderInfo {
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub dims: [u32; 3],
    pub scene_name: String,
    pub global_hit_ratio: f32,
}

impl PrtProbeVolume {
    /// Parse only the 116-byte `PRPV` v2 header (e.g. for Asset Inspector summary).
    pub fn parse_header(bytes: &[u8]) -> Result<PrtHeaderInfo, String> {
        if bytes.len() < PRPV_HEADER_SIZE {
            return Err(format!(
                "PRPV header truncated ({} < {PRPV_HEADER_SIZE} bytes)",
                bytes.len()
            ));
        }
        if &bytes[0..4] != PRPV_MAGIC {
            return Err(format!("invalid PRPV magic: {:?}", &bytes[0..4]));
        }
        let read_u32 = |slice: &[u8]| u32::from_le_bytes(slice.try_into().unwrap());
        let read_f32 = |slice: &[u8]| f32::from_le_bytes(slice.try_into().unwrap());

        let version = read_u32(&bytes[4..8]);
        if version != PRPV_VERSION {
            return Err(format!(
                "unsupported PRPV version {version} (expected {PRPV_VERSION})"
            ));
        }
        let origin = [
            read_f32(&bytes[8..12]),
            read_f32(&bytes[12..16]),
            read_f32(&bytes[16..20]),
        ];
        let spacing = [
            read_f32(&bytes[20..24]),
            read_f32(&bytes[24..28]),
            read_f32(&bytes[28..32]),
        ];
        let dims = [
            read_u32(&bytes[32..36]),
            read_u32(&bytes[36..40]),
            read_u32(&bytes[40..44]),
        ];
        let format = read_u32(&bytes[44..48]);
        if format != PRPV_FORMAT_F32 {
            return Err(format!("unsupported PRPV coeff format {format}"));
        }
        if dims.contains(&0) {
            return Err("PRPV dims must all be >= 1".to_string());
        }
        let raw_name = &bytes[48..48 + PRPV_SCENE_NAME_LEN];
        let end = raw_name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(PRPV_SCENE_NAME_LEN);
        let scene_name = String::from_utf8_lossy(&raw_name[..end]).into_owned();
        let global_hit_ratio = read_f32(&bytes[112..116]);

        Ok(PrtHeaderInfo {
            origin,
            spacing,
            dims,
            scene_name,
            global_hit_ratio,
        })
    }

    /// Decode a full `PRPV` v2 binary probe volume from disk bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let header = Self::parse_header(bytes)?;
        let probe_count = (header.dims[0] as usize)
            .checked_mul(header.dims[1] as usize)
            .and_then(|v| v.checked_mul(header.dims[2] as usize))
            .ok_or_else(|| "PRPV dims overflow".to_string())?;
        let coeff_count = probe_count
            .checked_mul(9)
            .ok_or_else(|| "PRPV coeff count overflow".to_string())?;
        let expected_body = coeff_count
            .checked_mul(12)
            .ok_or_else(|| "PRPV byte size overflow".to_string())?;
        if bytes.len() < PRPV_HEADER_SIZE + expected_body {
            return Err(format!(
                "PRPV body truncated ({} < {} bytes)",
                bytes.len(),
                PRPV_HEADER_SIZE + expected_body
            ));
        }

        let body = &bytes[PRPV_HEADER_SIZE..PRPV_HEADER_SIZE + expected_body];
        let mut coeffs = Vec::with_capacity(coeff_count);
        let read_f32 = |slice: &[u8]| f32::from_le_bytes(slice.try_into().unwrap());
        for i in 0..coeff_count {
            let base = i * 12;
            coeffs.push([
                read_f32(&body[base..base + 4]),
                read_f32(&body[base + 4..base + 8]),
                read_f32(&body[base + 8..base + 12]),
            ]);
        }

        Ok(Self {
            origin: header.origin,
            spacing: header.spacing,
            dims: header.dims,
            coeffs,
            scene_name: header.scene_name,
            global_hit_ratio: header.global_hit_ratio,
        })
    }

    /// Total probe count (`dims[0] * dims[1] * dims[2]`).
    pub fn probe_count(&self) -> usize {
        self.dims[0] as usize * self.dims[1] as usize * self.dims[2] as usize
    }

    /// Linear row-major index matching the GPU 3D texture upload layout.
    pub fn probe_index(&self, x: u32, y: u32, z: u32) -> usize {
        x as usize
            + y as usize * self.dims[0] as usize
            + z as usize * (self.dims[0] as usize * self.dims[1] as usize)
    }

    /// World position of probe `(x, y, z)`.
    pub fn probe_world_pos(&self, x: u32, y: u32, z: u32) -> [f32; 3] {
        [
            self.origin[0] + x as f32 * self.spacing[0],
            self.origin[1] + y as f32 * self.spacing[1],
            self.origin[2] + z as f32 * self.spacing[2],
        ]
    }

    /// Slice of 9 RGB SH coefficients for probe `probe_idx`.
    pub fn probe_sh(&self, probe_idx: usize) -> &[[f32; 3]; 9] {
        let start = probe_idx * 9;
        self.coeffs[start..start + 9]
            .try_into()
            .expect("PRPV probe coeff slice always has length 9")
    }

    /// Min and max world-space corners spanned by all probe centers.
    pub fn world_bounds(&self) -> ([f32; 3], [f32; 3]) {
        let max_x = self.origin[0] + (self.dims[0].saturating_sub(1)) as f32 * self.spacing[0];
        let max_y = self.origin[1] + (self.dims[1].saturating_sub(1)) as f32 * self.spacing[1];
        let max_z = self.origin[2] + (self.dims[2].saturating_sub(1)) as f32 * self.spacing[2];
        let min = [
            self.origin[0].min(max_x),
            self.origin[1].min(max_y),
            self.origin[2].min(max_z),
        ];
        let max = [
            self.origin[0].max(max_x),
            self.origin[1].max(max_y),
            self.origin[2].max(max_z),
        ];
        (min, max)
    }

    /// Order-2 real SH basis values matching `shaders/slang/gi.slang` and `prism-render/src/gi.rs`.
    pub fn sh_basis(n: [f32; 3]) -> [f32; 9] {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        let (x, y, z) = (n[0] / len, n[1] / len, n[2] / len);
        [
            SH_C0,
            SH_C1 * y,
            SH_C1 * z,
            SH_C1 * x,
            SH_C2 * x * y,
            SH_C2 * y * z,
            SH_C3 * (3.0 * z * z - 1.0),
            SH_C2 * x * z,
            SH_C4 * (x * x - y * y),
        ]
    }

    /// Evaluate order-2 SH irradiance along surface normal `n` (cosine convolution baked in).
    pub fn eval_sh9_irradiance(n: [f32; 3], sh: &[[f32; 3]; 9]) -> [f32; 3] {
        let b = Self::sh_basis(n);
        let al = [A_L0, A_L1, A_L1, A_L1, A_L2, A_L2, A_L2, A_L2, A_L2];
        let mut out = [0.0f32; 3];
        for c in 0..9 {
            let w = b[c] * al[c];
            out[0] += sh[c][0] * w;
            out[1] += sh[c][1] * w;
            out[2] += sh[c][2] * w;
        }
        [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
    }

    /// Reconstruct isotropic DC irradiance (average over sphere) for probe `probe_idx`.
    pub fn probe_mean_irradiance(&self, probe_idx: usize) -> [f32; 3] {
        let sh = self.probe_sh(probe_idx);
        let scale = SH_C0 * A_L0;
        [
            (sh[0][0] * scale).max(0.0),
            (sh[0][1] * scale).max(0.0),
            (sh[0][2] * scale).max(0.0),
        ]
    }

    /// Extract the dominant incoming light direction and anisotropy magnitude `[0..]` from L1 SH.
    pub fn dominant_direction(sh: &[[f32; 3]; 9]) -> ([f32; 3], f32) {
        let luma = |rgb: [f32; 3]| rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
        // L1 basis order in PRPV: idx 1 = Y_1^-1 (y), idx 2 = Y_1^0 (z), idx 3 = Y_1^1 (x).
        let dir = [luma(sh[3]), luma(sh[1]), luma(sh[2])];
        let mag = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if mag < 1e-6 {
            return ([0.0, 1.0, 0.0], 0.0);
        }
        let dc = luma(sh[0]).abs().max(1e-4);
        ([dir[0] / mag, dir[1] / mag, dir[2] / mag], mag / dc)
    }

    /// Tone-map HDR linear irradiance (divided by PI for Lambert preview) into 8-bit sRGB.
    pub fn irradiance_to_srgb8(irradiance: [f32; 3], exposure: f32) -> [u8; 3] {
        let exp = exposure.max(0.01) / PI;
        let to_srgb = |v: f32| -> u8 {
            let x = (v * exp).max(0.0);
            let mapped = x / (1.0 + x);
            let gamma = mapped.powf(1.0 / 2.2);
            (gamma.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        [
            to_srgb(irradiance[0]),
            to_srgb(irradiance[1]),
            to_srgb(irradiance[2]),
        ]
    }

    /// Trilinearly sample the 8 surrounding probes around `world_pos`, returning corner weights
    /// and the interpolated SH9 matrix. Returns `None` if spacing is degenerate.
    pub fn trilinear_sample(
        &self,
        world_pos: [f32; 3],
    ) -> Option<(Vec<(usize, [f32; 3], f32)>, [[f32; 3]; 9])> {
        if self.spacing[0].abs() < 1e-6
            || self.spacing[1].abs() < 1e-6
            || self.spacing[2].abs() < 1e-6
        {
            return None;
        }
        let gx = ((world_pos[0] - self.origin[0]) / self.spacing[0])
            .clamp(0.0, (self.dims[0] - 1) as f32);
        let gy = ((world_pos[1] - self.origin[1]) / self.spacing[1])
            .clamp(0.0, (self.dims[1] - 1) as f32);
        let gz = ((world_pos[2] - self.origin[2]) / self.spacing[2])
            .clamp(0.0, (self.dims[2] - 1) as f32);

        let x0 = (gx.floor() as u32).min(self.dims[0] - 1);
        let y0 = (gy.floor() as u32).min(self.dims[1] - 1);
        let z0 = (gz.floor() as u32).min(self.dims[2] - 1);
        let x1 = (x0 + 1).min(self.dims[0] - 1);
        let y1 = (y0 + 1).min(self.dims[1] - 1);
        let z1 = (z0 + 1).min(self.dims[2] - 1);

        let fx = gx - x0 as f32;
        let fy = gy - y0 as f32;
        let fz = gz - z0 as f32;

        let mut corner_map: HashMap<usize, ([f32; 3], f32)> = HashMap::with_capacity(8);
        let mut blended = [[0.0f32; 3]; 9];
        for (ix, wx) in [(x0, 1.0 - fx), (x1, fx)] {
            for (iy, wy) in [(y0, 1.0 - fy), (y1, fy)] {
                for (iz, wz) in [(z0, 1.0 - fz), (z1, fz)] {
                    let w = wx * wy * wz;
                    if w <= 0.0 {
                        continue;
                    }
                    let idx = self.probe_index(ix, iy, iz);
                    let pos = self.probe_world_pos(ix, iy, iz);
                    let sh = self.probe_sh(idx);
                    for c in 0..9 {
                        blended[c][0] += sh[c][0] * w;
                        blended[c][1] += sh[c][1] * w;
                        blended[c][2] += sh[c][2] * w;
                    }
                    corner_map
                        .entry(idx)
                        .and_modify(|e| e.1 += w)
                        .or_insert((pos, w));
                }
            }
        }

        let mut corners: Vec<(usize, [f32; 3], f32)> = corner_map
            .into_iter()
            .filter(|(_, (_, w))| *w > 1e-4)
            .map(|(idx, (pos, w))| (idx, pos, w))
            .collect();
        corners.sort_by(|a, b| b.2.total_cmp(&a.2));
        Some((corners, blended))
    }
}

/// Choose an integer step `>= 1` per axis so drawn probe count stays <= `max_probes`.
fn compute_probe_stride(dims: [u32; 3], slice_y: Option<u32>, max_probes: usize) -> [u32; 3] {
    let cap = max_probes.max(1) as f32;
    let ny = if slice_y.is_some() { 1 } else { dims[1].max(1) };
    let total = dims[0] as f32 * ny as f32 * dims[2] as f32;
    if total <= cap {
        return [1, 1, 1];
    }
    let axes = if slice_y.is_some() { 2.0 } else { 3.0 };
    let stride = (total / cap).powf(1.0 / axes).ceil().max(1.0) as u32;
    [stride, if slice_y.is_some() { 1 } else { stride }, stride]
}

/// Emit PRT probe-volume Scene View gizmos (bounds, probes, SH lighting & trilinear tethers).
pub fn emit_prt_probe_gizmos(
    volume: &PrtProbeVolume,
    settings: &PrtGizmoSettings,
    focus_world: Option<[f32; 3]>,
    camera_pos: [f32; 3],
    gizmos: &mut GizmoContext,
) {
    if !settings.enabled {
        return;
    }

    let (bmin, bmax) = volume.world_bounds();
    let center = [
        (bmin[0] + bmax[0]) * 0.5,
        (bmin[1] + bmax[1]) * 0.5,
        (bmin[2] + bmax[2]) * 0.5,
    ];
    let half = [
        ((bmax[0] - bmin[0]) * 0.5).max(0.05),
        ((bmax[1] - bmin[1]) * 0.5).max(0.05),
        ((bmax[2] - bmin[2]) * 0.5).max(0.05),
    ];

    if settings.show_grid_bounds || settings.mode == PrtDisplayMode::GridBoundsOnly {
        gizmos.draw_wire_box(center, half, [0.0, 0.0, 0.0, 1.0], [78, 200, 228, 175], 1.2);
    }

    // Draw the active Y-slice horizontal highlight rectangle if slicing by layer.
    if let Some(layer) = settings.slice_y {
        let y_idx = layer.min(volume.dims[1].saturating_sub(1));
        let y_world = volume.origin[1] + y_idx as f32 * volume.spacing[1];
        gizmos.draw_wire_box(
            [center[0], y_world, center[2]],
            [half[0], 0.002, half[2]],
            [0.0, 0.0, 0.0, 1.0],
            [245, 198, 72, 185],
            1.2,
        );
    }

    if settings.mode == PrtDisplayMode::GridBoundsOnly {
        return;
    }

    let trilinear = focus_world.and_then(|pos| volume.trilinear_sample(pos));
    let active_indices: HashMap<usize, f32> = trilinear
        .as_ref()
        .map(|(corners, _)| corners.iter().map(|&(idx, _, w)| (idx, w)).collect())
        .unwrap_or_default();

    let stride = compute_probe_stride(volume.dims, settings.slice_y, settings.max_probes);
    let y_range = match settings.slice_y {
        Some(y) => {
            let clamped = y.min(volume.dims[1].saturating_sub(1));
            clamped..=clamped
        }
        None => 0..=volume.dims[1].saturating_sub(1),
    };

    // Collect (dist_sq_from_camera, probe_idx, pos) so far probes render before near probes.
    let mut selected_probes: Vec<(f32, usize, [f32; 3])> = Vec::new();
    for y in y_range {
        let y_step_ok = settings.slice_y.is_some() || y % stride[1] == 0 || y + 1 == volume.dims[1];
        for z in 0..volume.dims[2] {
            let z_step_ok = z % stride[2] == 0 || z + 1 == volume.dims[2];
            for x in 0..volume.dims[0] {
                let x_step_ok = x % stride[0] == 0 || x + 1 == volume.dims[0];
                let idx = volume.probe_index(x, y, z);
                let highlighted = active_indices.contains_key(&idx);
                if !highlighted && !(x_step_ok && y_step_ok && z_step_ok) {
                    continue;
                }
                let pos = volume.probe_world_pos(x, y, z);
                let dx = pos[0] - camera_pos[0];
                let dy = pos[1] - camera_pos[1];
                let dz = pos[2] - camera_pos[2];
                let dist_sq = dx * dx + dy * dy + dz * dz;
                selected_probes.push((dist_sq, idx, pos));
            }
        }
    }
    selected_probes.sort_by(|a, b| b.0.total_cmp(&a.0));

    let min_spacing = volume.spacing[0]
        .abs()
        .min(volume.spacing[1].abs())
        .min(volume.spacing[2].abs())
        .max(0.25);
    let radius = settings.probe_radius.clamp(0.04, min_spacing * 0.42);
    let dir_len = min_spacing * 0.45;

    for (_, idx, pos) in selected_probes {
        let weight = active_indices.get(&idx).copied();
        let outline = weight.map(|w| {
            let intensity = (120.0 + w * 135.0).clamp(120.0, 255.0) as u8;
            [250, 210, 82, intensity]
        });
        let sh = *volume.probe_sh(idx);

        match settings.mode {
            PrtDisplayMode::ShSpheres => {
                gizmos.draw_sh_probe_sphere(pos, radius, sh, settings.exposure, outline);
            }
            PrtDisplayMode::MeanIrradiance | PrtDisplayMode::DominantDirection => {
                let rgb = PrtProbeVolume::irradiance_to_srgb8(
                    volume.probe_mean_irradiance(idx),
                    settings.exposure,
                );
                gizmos.draw_solid_sphere(pos, radius, [rgb[0], rgb[1], rgb[2], 255], outline);
                if settings.mode == PrtDisplayMode::DominantDirection {
                    let (dir, strength) = PrtProbeVolume::dominant_direction(&sh);
                    if strength > 0.03 {
                        let scale = (0.35 + strength * 0.65).clamp(0.35, 1.2) * dir_len;
                        let tip = [
                            pos[0] + dir[0] * scale,
                            pos[1] + dir[1] * scale,
                            pos[2] + dir[2] * scale,
                        ];
                        gizmos.draw_line(pos, tip, [252, 224, 110, 230], 1.6);
                    }
                }
            }
            PrtDisplayMode::GridBoundsOnly => {}
        }
    }

    // Trilinear tethers connecting the focused/selected world position to its 8 probes.
    if settings.show_trilinear_tethers {
        if let (Some(target), Some((corners, blended_sh))) = (focus_world, trilinear) {
            for (_, probe_pos, weight) in corners {
                let alpha = (70.0 + weight * 185.0).clamp(70.0, 255.0) as u8;
                gizmos.draw_line(
                    target,
                    probe_pos,
                    [90, 225, 245, alpha],
                    if weight > 0.25 { 1.8 } else { 1.1 },
                );
            }
            if settings.mode == PrtDisplayMode::ShSpheres {
                gizmos.draw_sh_probe_sphere(
                    target,
                    radius * 1.15,
                    blended_sh,
                    settings.exposure,
                    Some([255, 255, 255, 230]),
                );
            }
        }
    }
}

/// Generate a unit-circle lookup table in the local UV plane (`segments` points).
pub fn unit_ring_points(segments: usize) -> Vec<[f32; 2]> {
    let n = segments.max(8);
    (0..n)
        .map(|i| {
            let a = i as f32 * TAU / n as f32;
            [a.cos(), a.sin()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_prpv(
        origin: [f32; 3],
        spacing: [f32; 3],
        dims: [u32; 3],
        scene_name: &str,
        hit_ratio: f32,
        probe_coeffs: &[[[f32; 3]; 9]],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(PRPV_MAGIC);
        bytes.extend_from_slice(&PRPV_VERSION.to_le_bytes());
        for f in origin.into_iter().chain(spacing) {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        for u in dims {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        bytes.extend_from_slice(&PRPV_FORMAT_F32.to_le_bytes());
        let mut name_buf = [0u8; PRPV_SCENE_NAME_LEN];
        let copy_len = scene_name.len().min(PRPV_SCENE_NAME_LEN);
        name_buf[..copy_len].copy_from_slice(&scene_name.as_bytes()[..copy_len]);
        bytes.extend_from_slice(&name_buf);
        bytes.extend_from_slice(&hit_ratio.to_le_bytes());
        for sh in probe_coeffs {
            for rgb in sh {
                for f in rgb {
                    bytes.extend_from_slice(&f.to_le_bytes());
                }
            }
        }
        bytes
    }

    #[test]
    fn prpv_binary_roundtrip_and_sh_irradiance() {
        let mut sh0 = [[0.0f32; 3]; 9];
        // Pure isotropic irradiance E = [PI, PI*0.5, 0] requires c0 = E / (SH_C0 * PI) = 1 / SH_C0.
        sh0[0] = [1.0 / SH_C0, 0.5 / SH_C0, 0.0];
        let mut sh1 = sh0;
        sh1[2] = [0.5 / SH_C1, 0.5 / SH_C1, 0.5 / SH_C1];

        let raw = make_test_prpv(
            [-1.0, 0.0, -1.0],
            [2.0, 2.0, 2.0],
            [2, 1, 1],
            "default",
            0.85,
            &[sh0, sh1],
        );
        let volume = PrtProbeVolume::from_bytes(&raw).expect("valid PRPV binary");
        assert_eq!(volume.dims, [2, 1, 1]);
        assert_eq!(volume.probe_count(), 2);
        assert_eq!(volume.scene_name, "default");
        assert!((volume.global_hit_ratio - 0.85).abs() < 1e-6);

        let mean0 = volume.probe_mean_irradiance(0);
        assert!((mean0[0] - PI).abs() < 1e-4);
        assert!((mean0[1] - PI * 0.5).abs() < 1e-4);
        assert!((mean0[2]).abs() < 1e-4);

        let irr_up = PrtProbeVolume::eval_sh9_irradiance([0.0, 1.0, 0.0], volume.probe_sh(0));
        assert!((irr_up[0] - PI).abs() < 1e-4);

        let (dir, strength) = PrtProbeVolume::dominant_direction(volume.probe_sh(1));
        assert!(dir[2] > 0.99);
        assert!(strength > 0.1);
    }

    #[test]
    fn prpv_rejects_bad_magic_and_truncated_body() {
        assert!(PrtProbeVolume::from_bytes(b"BAD!").is_err());
        let mut raw = make_test_prpv([0.0; 3], [1.0; 3], [2, 2, 2], "s", 1.0, &[[[0.0; 3]; 9]; 8]);
        raw.truncate(PRPV_HEADER_SIZE + 12);
        assert!(PrtProbeVolume::from_bytes(&raw).is_err());
    }

    #[test]
    fn trilinear_sample_weights_sum_to_one_and_midpoint_averages() {
        let mut c0 = [[0.0f32; 3]; 9];
        c0[0] = [1.0, 0.0, 0.0];
        let mut c1 = [[0.0f32; 3]; 9];
        c1[0] = [0.0, 1.0, 0.0];
        let mut coeffs = vec![[[0.0f32; 3]; 9]; 8];
        coeffs[0] = c0; // (0,0,0)
        coeffs[1] = c1; // (1,0,0)
        let raw = make_test_prpv(
            [0.0, 0.0, 0.0],
            [2.0, 2.0, 2.0],
            [2, 2, 2],
            "grid",
            1.0,
            &coeffs,
        );
        let volume = PrtProbeVolume::from_bytes(&raw).unwrap();

        let (corners, blended) = volume
            .trilinear_sample([1.0, 0.0, 0.0])
            .expect("midpoint valid");
        let sum_w: f32 = corners.iter().map(|c| c.2).sum();
        assert!((sum_w - 1.0).abs() < 1e-5);
        assert!((blended[0][0] - 0.5).abs() < 1e-5);
        assert!((blended[0][1] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn emit_prt_gizmos_subsamples_large_grid_and_includes_tethers() {
        let coeffs = vec![[[0.5f32; 3]; 9]; 10 * 4 * 10];
        let raw = make_test_prpv(
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [10, 4, 10],
            "large",
            1.0,
            &coeffs,
        );
        let volume = PrtProbeVolume::from_bytes(&raw).unwrap();
        let settings = PrtGizmoSettings {
            enabled: true,
            max_probes: 64,
            ..PrtGizmoSettings::default()
        };
        let mut ctx = GizmoContext::new();
        emit_prt_probe_gizmos(
            &volume,
            &settings,
            Some([2.3, 1.2, 4.7]),
            [0.0, 5.0, 10.0],
            &mut ctx,
        );
        let sh_count = ctx
            .commands()
            .iter()
            .filter(|cmd| matches!(cmd, GizmoCommand::ShProbeSphere { .. }))
            .count();
        assert!(sh_count > 8 && sh_count <= 160, "got {sh_count}");
        assert!(ctx
            .commands()
            .iter()
            .any(|cmd| matches!(cmd, GizmoCommand::Line { .. })));
        assert!(ctx
            .commands()
            .iter()
            .any(|cmd| matches!(cmd, GizmoCommand::WireBox { .. })));
    }

    #[test]
    fn gizmo_registry_collects_builtin_components_and_custom_extensions() {
        let mut doc = SceneDoc::default();
        let cam = doc.add_named_child(None, "Main Camera");
        doc.entities[cam].components.insert(
            COMPONENT_CAMERA.to_string(),
            serde_json::json!({"fov_y_degrees": 60.0}),
        );
        let col = doc.add_named_child(None, "ColliderBox");
        doc.entities[col].components.insert(
            COMPONENT_COLLIDER.to_string(),
            serde_json::json!({"shape": "Box", "half_extents": [1.0, 2.0, 3.0]}),
        );
        doc.entities[col].components.insert(
            "my_game::Waypoint".to_string(),
            serde_json::json!({"radius": 2.0}),
        );

        let mut registry = GizmoRegistry::with_builtins();
        registry.register_fn(
            "my_game::Waypoint",
            GizmoCategory::Custom("wp"),
            |ctx, g| {
                g.draw_wire_sphere(
                    ctx.position,
                    2.0,
                    [0.0, 0.0, 0.0, 1.0],
                    [255, 100, 100, 255],
                    1.5,
                );
            },
        );

        let prt = PrtGizmoSettings::default();
        let filter = GizmoFilter {
            show_cameras: true,
            show_lights: true,
            show_colliders: true,
            show_audio: true,
            prt: &prt,
        };
        let mut ctx = GizmoContext::new();
        registry.collect_scene(&doc, &[cam], &[], None, 16.0 / 9.0, 1.5, &filter, &mut ctx);

        assert!(ctx
            .commands()
            .iter()
            .any(|c| matches!(c, GizmoCommand::WireFrustum { .. })));
        assert!(ctx
            .commands()
            .iter()
            .any(|c| matches!(c, GizmoCommand::WireBox { .. })));
        assert!(ctx
            .commands()
            .iter()
            .any(|c| matches!(c, GizmoCommand::WireSphere { .. })));
    }
}
