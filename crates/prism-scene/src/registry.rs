//! ComponentRegistry — generic component deserialization registry.
//!
//! Each entity in scene file (.scene.json) carries a component list,
//! each component identified by its type name (fully qualified path). At runtime load
//! [`ComponentRegistry`] finds matching deserialization factory, restores JSON data to
//! ECS component and inserts into entity.
//!
//! Built-in components registered in batch via [`register_builtin_components`].

use std::collections::HashMap;

use prism_ecs::{Entity, World};
use prism_reflect::{Object, Reflect, ReflectError, TypeDesc, TypeId, TypeRegistry, Value};
use serde::{de::DeserializeOwned, Serialize};

// ---------------------------------------------------------------------------
// ComponentRegistry
// ---------------------------------------------------------------------------

type DeserializerFn = Box<dyn Fn(&mut World, Entity, &serde_json::Value) + Send + Sync>;

type JsonToObjectFn = Box<dyn Fn(&serde_json::Value) -> Option<Object> + Send + Sync>;

/// Produce **default author JSON shape** of some component (for editor "add component").
type DefaultJsonFn = Box<dyn Fn() -> serde_json::Value + Send + Sync>;

/// Reflection object → author JSON (for editor "modify fields based on reflection then write back").
type ObjectToJsonFn = Box<dyn Fn(&Object) -> Option<serde_json::Value> + Send + Sync>;

type ReflectApplyFn = fn(&mut World, Entity, &Object) -> bool;
type ReflectExtractFn = fn(&World, Entity) -> Option<Object>;
/// Write fields by name on reflection object (`Object -> C -> set_field -> Object`).
type ReflectSetFn = fn(&mut Object, &str, Value) -> Result<(), ReflectError>;
/// List entities owning that component (type-erased `World::query`).
type ReflectQueryFn = fn(&World) -> Vec<Entity>;
/// Remove that component and report whether it existed (type-erased `World::remove`).
type ReflectRemoveFn = fn(&mut World, Entity) -> bool;

/// Reflection adapter for one component: `Object <-> concrete ECS component`.
struct ReflectEntry {
    desc: &'static TypeDesc,
    apply: ReflectApplyFn,
    extract: ReflectExtractFn,
    set_field: ReflectSetFn,
    query: ReflectQueryFn,
    remove: ReflectRemoveFn,
}

/// Component deserialization registry.
///
/// Also holds a **reflection path**: `prism-reflect`'s `Object` can bypass serde,
/// restoring directly into component. Latter is generic entry for RTTIBin / editor, former kept for
/// `.scene.json` author format.
pub struct ComponentRegistry {
    deserializers: HashMap<String, DeserializerFn>,
    reflect: TypeRegistry,
    reflect_entries: HashMap<TypeId, ReflectEntry>,
    name_to_id: HashMap<String, TypeId>,
    json_to_object: HashMap<String, JsonToObjectFn>,
    object_to_json: HashMap<String, ObjectToJsonFn>,
    defaults: HashMap<String, DefaultJsonFn>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self {
            deserializers: HashMap::new(),
            reflect: TypeRegistry::new(),
            reflect_entries: HashMap::new(),
            name_to_id: HashMap::new(),
            json_to_object: HashMap::new(),
            object_to_json: HashMap::new(),
            defaults: HashMap::new(),
        }
    }

    /// Register a serde component type.
    pub fn register<C>(&mut self, name: &str)
    where
        C: DeserializeOwned + Send + Sync + 'static,
    {
        let name = name.to_string();
        self.deserializers.insert(
            name,
            Box::new(move |world, entity, value| {
                match serde_json::from_value::<C>(value.clone()) {
                    Ok(comp) => {
                        world.insert(entity, comp);
                    }
                    Err(e) => {
                        log::warn!(
                            "ComponentRegistry: failed to deserialize '{}': {e}",
                            std::any::type_name::<C>(),
                        );
                    }
                }
            }),
        );
    }

    /// Register a custom deserializer.
    pub fn register_with<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut World, Entity, &serde_json::Value) + 'static + Send + Sync,
    {
        self.deserializers.insert(name.to_string(), Box::new(f));
    }

    /// Register a reflection component type.
    ///
    /// Type name taken from `#[reflect(name = "...")]` (should match component name in `.scene.json`),
    /// so caller need not pass name again.
    pub fn register_reflect<C>(&mut self)
    where
        C: Reflect + Send + Sync,
    {
        let desc = C::type_desc();
        self.reflect.register::<C>();
        self.name_to_id.insert(desc.name.to_owned(), desc.id);
        self.reflect_entries.insert(
            desc.id,
            ReflectEntry {
                desc,
                apply: |world, entity, object| match <C as Reflect>::from_object(object) {
                    Ok(component) => {
                        world.insert(entity, component);
                        true
                    }
                    Err(err) => {
                        log::warn!(
                            "ComponentRegistry: failed to reflect-restore '{}': {err}",
                            <C as Reflect>::type_desc().name,
                        );
                        false
                    }
                },
                extract: |world, entity| {
                    world
                        .get::<C>(entity)
                        .map(|component| <C as Reflect>::to_object(component))
                },
                set_field: |object, field, value| {
                    let mut component = <C as Reflect>::from_object(object)?;
                    component.set_field(field, value)?;
                    *object = component.to_object();
                    Ok(())
                },
                query: |world| world.query::<C>().map(|(entity, _)| entity).collect(),
                remove: |world, entity| world.remove::<C>(entity).is_some(),
            },
        );
    }

    /// Write fields by name on reflection object, return object after writing.
    ///
    /// Goes through [`Reflect::set_field`]: `#[reflect(read_only)]` fields rejected,
    /// value type must also be field's declared reflection type. If type not registered returns
    /// [`ReflectError::NotRegistered`].
    pub fn set_object_field(
        &self,
        object: &mut Object,
        field: &str,
        value: Value,
    ) -> Result<(), ReflectError> {
        let entry = self
            .reflect_entries
            .get(&object.type_id)
            .ok_or(ReflectError::NotRegistered(object.type_id))?;
        (entry.set_field)(object, field, value)
    }

    /// Register a component supporting both serde deserialization and reflection.
    ///
    /// `name` must match `#[reflect(name = "...")]` (key of scene JSON).
    /// Also registers JSON → `Object` converter for cook tool converting author JSON
    /// into compact RSCN payload.
    pub fn register_reflect_serde<C>(&mut self, name: &str)
    where
        C: Reflect + DeserializeOwned + Serialize + Send + Sync + 'static,
    {
        self.register::<C>(name);
        self.register_reflect::<C>();
        let key = name.to_string();
        self.json_to_object.insert(
            key.clone(),
            Box::new(move |value| {
                serde_json::from_value::<C>(value.clone())
                    .ok()
                    .map(|component| <C as Reflect>::to_object(&component))
            }),
        );
        // Default author JSON for editor "add component": serde shape of `C::default()`.
        self.defaults.insert(
            key.clone(),
            Box::new(|| serde_json::to_value(C::default()).unwrap_or(serde_json::Value::Null)),
        );
        // Reflection object → author JSON (`Object -> C -> serde JSON`).
        self.object_to_json.insert(
            key,
            Box::new(|object| {
                let component = <C as Reflect>::from_object(object).ok()?;
                serde_json::to_value(component).ok()
            }),
        );
    }

    /// Manually register "reflection object → author JSON" converter.
    ///
    /// For components like `Name` whose serde shape differs from derived default.
    pub fn register_object_to_json<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&Object) -> Option<serde_json::Value> + Send + Sync + 'static,
    {
        self.object_to_json.insert(name.to_string(), Box::new(f));
    }

    /// Reflection object → author JSON (returns `None` if not registered).
    pub fn object_to_json(&self, name: &str, object: &Object) -> Option<serde_json::Value> {
        self.object_to_json.get(name).and_then(|f| f(object))
    }

    /// Manually register default author JSON shape for some component.
    ///
    /// For components like `Name` whose serde shape differs from derived default
    /// (`register_reflect_serde` cannot cover).
    pub fn set_default_json(&mut self, name: &str, value: serde_json::Value) {
        self.defaults
            .insert(name.to_string(), Box::new(move || value.clone()));
    }

    /// Default author JSON shape of some component (for editor "add component").
    ///
    /// Returns `None` if not registered; caller can fall back to empty `serde_json::Value::Object`.
    pub fn default_json(&self, name: &str) -> Option<serde_json::Value> {
        self.defaults.get(name).map(|f| f())
    }

    /// Type names of all registered components (stable order, for editor component list).
    pub fn component_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .reflect_entries
            .values()
            .map(|entry| entry.desc.name)
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Register a custom JSON → `Object` converter for an already reflection-registered component.
    ///
    /// For components like `Name` whose JSON shape differs from derived serde default.
    pub fn register_json_object<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&serde_json::Value) -> Option<Object> + Send + Sync + 'static,
    {
        self.json_to_object.insert(name.to_string(), Box::new(f));
    }

    /// Convert author JSON to reflection object (cook tool path).
    pub fn object_from_json(&self, name: &str, value: &serde_json::Value) -> Option<Object> {
        self.json_to_object.get(name).and_then(|f| f(value))
    }

    /// Register type description only (not as component).
    ///
    /// For nested types appearing in component fields (like `glam::Vec3` / `glam::Quat`):
    /// `rtti_bin::encode` collecting type table requires these types also in `TypeRegistry`.
    pub fn register_reflect_type<T: Reflect>(&mut self) {
        self.reflect.register::<T>();
    }

    /// Deserialize and insert one component.
    pub fn apply(&self, world: &mut World, entity: Entity, name: &str, data: &serde_json::Value) {
        match self.deserializers.get(name) {
            Some(f) => f(world, entity, data),
            None => {
                log::warn!("ComponentRegistry: unknown component '{name}' — skipping");
            }
        }
    }

    /// Insert component with reflection object (RTTIBin / editor path).
    ///
    /// Returns `false` meaning type not registered or field restore failed.
    pub fn apply_object(&self, world: &mut World, entity: Entity, object: &Object) -> bool {
        match self.reflect_entries.get(&object.type_id) {
            Some(entry) => (entry.apply)(world, entity, object),
            None => {
                log::warn!(
                    "ComponentRegistry: unknown reflected component '{}' ({}) — skipping",
                    object.type_name,
                    object.type_id,
                );
                false
            }
        }
    }

    /// Extract reflection object from entity (for save / editor).
    pub fn extract_object(&self, world: &World, entity: Entity, type_id: TypeId) -> Option<Object> {
        let entry = self.reflect_entries.get(&type_id)?;
        (entry.extract)(world, entity)
    }

    /// Extract reflection object by type name.
    pub fn extract_object_by_name(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<Object> {
        let type_id = self.name_to_id.get(name)?;
        self.extract_object(world, entity, *type_id)
    }

    /// Type name → stable type id.
    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.name_to_id.get(name).copied()
    }

    /// List entities owning component `type_id` (type-erased, for host ABI).
    ///
    /// Returns `None` meaning type has no registered reflection component.
    pub fn entities_with(&self, world: &World, type_id: TypeId) -> Option<Vec<Entity>> {
        let entry = self.reflect_entries.get(&type_id)?;
        Some((entry.query)(world))
    }

    /// Remove component by `type_id`, return whether it existed.
    ///
    /// Returns `false` when type not registered (same as "component does not exist", caller need not distinguish).
    pub fn remove_component(&self, world: &mut World, entity: Entity, type_id: TypeId) -> bool {
        match self.reflect_entries.get(&type_id) {
            Some(entry) => (entry.remove)(world, entity),
            None => false,
        }
    }

    /// Type description (for editor property panel / schema browsing).
    pub fn descriptor(&self, type_id: TypeId) -> Option<&'static TypeDesc> {
        self.reflect_entries.get(&type_id).map(|entry| entry.desc)
    }

    /// Underlying `TypeRegistry`, for `rtti_bin::encode` collecting type table.
    pub fn type_registry(&self) -> &TypeRegistry {
        &self.reflect
    }

    /// Number of registered reflection components.
    pub fn reflect_component_count(&self) -> usize {
        self.reflect_entries.len()
    }

    /// Compute a deterministic schema fingerprint of all reflected components.
    ///
    /// Changes whenever any component is added, removed, or has its schema / fields
    /// changed (including nested types via `TypeDesc::schema_hash`).
    pub fn schema_fingerprint(&self) -> u64 {
        const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

        let mut entries: Vec<(&str, u64)> = self
            .reflect_entries
            .values()
            .map(|entry| {
                let hash = entry.desc.schema_hash(&self.reflect);
                (entry.desc.name, hash)
            })
            .collect();
        entries.sort_unstable_by_key(|&(name, _)| name);

        let mut hash = FNV_OFFSET_BASIS;
        for (name, schema_hash) in entries {
            for byte in name.as_bytes() {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(FNV_PRIME);
            }
            hash ^= schema_hash;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// built-in component register
// ---------------------------------------------------------------------------

/// The builtin registry, built once per process.
///
/// Authoring JSON, the cook tools, the engine and project modules all need the
/// same schema; sharing one instance keeps their fingerprints from drifting.
/// The registry is immutable after construction, so it can be shared across
/// threads. Hosts that add their own components still build a separate
/// registry with [`register_builtin_components`].
pub fn builtin_registry() -> &'static ComponentRegistry {
    static REGISTRY: std::sync::OnceLock<ComponentRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = ComponentRegistry::new();
        register_builtin_components(&mut registry);
        registry
    })
}

/// Returns the builtin components schema fingerprint.
pub fn builtin_schema_fingerprint() -> u64 {
    builtin_registry().schema_fingerprint()
}

/// Registers every builtin scene and UI component.
///
/// This is the single schema entry point shared by authoring JSON, the cook
/// tools and the runtime.
pub fn register_builtin_components(registry: &mut ComponentRegistry) {
    use crate::components::*;
    #[cfg(feature = "group-ui")]
    use crate::ui::{
        Anchors, Button, Canvas, ComputedLayout, Interaction, Layout, Margin, Node, Pivot, Style,
        Text,
    };

    // ── scene base components ──
    registry.register_reflect_serde::<Active>("prism_engine::scene::Active");

    // Name uses custom JSON shape (string, not {"0": ...}).
    registry.register_with("prism_engine::scene::Name", |world, entity, value| {
        if let Some(s) = value.as_str() {
            world.insert(entity, Name(s.to_string()));
        } else {
            log::warn!("ComponentRegistry: Name requires a string value, got {value:?}");
        }
    });
    registry.register_reflect::<Name>();
    registry.register_json_object("prism_engine::scene::Name", |value| {
        value.as_str().map(|s| Name(s.to_string()).to_object())
    });
    registry.set_default_json("prism_engine::scene::Name", serde_json::json!("entity"));
    registry.register_object_to_json("prism_engine::scene::Name", |object| {
        object
            .get("0")
            .and_then(|value| value.as_str().ok())
            .map(|name| serde_json::json!(name))
    });

    // ── light source  ──
    #[cfg(feature = "group-3d")]
    {
        registry.register_reflect_serde::<Light>("prism_engine::scene::Light");
        registry
            .register_reflect_serde::<DirectionalLight>("prism_engine::scene::DirectionalLight");
        registry.register_reflect_serde::<PointLight>("prism_engine::scene::PointLight");
        registry.register_reflect_serde::<SpotLight>("prism_engine::scene::SpotLight");

        // ── declarative lighting (scene-driven IBL / GI, avoid engine startup unconditional load) ──
        registry.register_reflect_serde::<EnvironmentLighting>(
            "prism_engine::scene::EnvironmentLighting",
        );
        registry.register_reflect_serde::<GiConfig>("prism_engine::scene::GiConfig");
        registry.register_reflect_serde::<ProbeVolume>("prism_engine::scene::ProbeVolume");
    }

    // ── camera  ──
    registry.register_reflect_serde::<Camera>("prism_engine::scene::Camera");
    #[cfg(feature = "group-2d")]
    registry.register_reflect_serde::<VirtualCamera2D>("prism_engine::scene::VirtualCamera2D");
    #[cfg(feature = "group-3d")]
    {
        registry.register_reflect_serde::<FlyCameraController>(
            "prism_engine::scene::FlyCameraController",
        );
    }

    // ── skybox / render bundle ──
    #[cfg(feature = "group-3d")]
    {
        registry.register_reflect_serde::<Skybox>("prism_engine::scene::Skybox");
        registry.register_reflect_serde::<Gizmo>("prism_engine::scene::Gizmo");
        registry.register_reflect_serde::<MeshRenderer>("prism_engine::scene::MeshRenderer");
        // #491: distance LOD (child entity is LOD level).
        registry.register_reflect_serde::<LodGroup>("prism_engine::scene::LodGroup");
    }

    // ── 2D sprite (author state; GPU handle held by runtime `SpriteRef`) ──
    #[cfg(feature = "group-2d")]
    {
        registry.register_reflect_serde::<SpriteRenderer>("prism_engine::scene::SpriteRenderer");
        registry.register_reflect_serde::<LineRenderer>("prism_engine::scene::LineRenderer");
        registry.register_reflect_serde::<SpriteAnimation>("prism_engine::scene::SpriteAnimation");
        registry.register_reflect_serde::<Tilemap>("prism_engine::scene::Tilemap");
        // #611: 2D sprite lighting (same group as SpriteRenderer, meaningless without sprite).
        registry.register_reflect_serde::<Light2D>("prism_engine::scene::Light2D");
        // #983: day-night clock. It only writes color into ambient light of same entity, 2D scenes use it often,
        // but can also do logical timing in pure 3D projects, so not limiting by `group-2d` is more sensible;
        // currently registered with Light2D, keeping consistency "no 2D means no day-night appearance".
        registry.register_reflect_serde::<TimeOfDay>("prism_engine::scene::TimeOfDay");
        // #1160: 2D camera and 2D physics
        registry.register_reflect_serde::<Camera2D>("prism_engine::scene::Camera2D");
        registry.register_reflect_serde::<RigidBody2D>("prism_engine::scene::RigidBody2D");
        registry
            .register_reflect_serde::<PhysicsCollider2D>("prism_engine::scene::PhysicsCollider2D");
    }

    // ── local transform  ──
    // Register **types only** (RTTIBin type table / editor field descriptions), not as attachable component:
    // Author only writes `EntityJson::transform`, baked by loader / cooker into
    // `LocalTransform` + `LocalToWorld` (the non-removable Transform in author state).
    // If registered as component, `registry.apply` would overwrite `LocalTransform` after
    // `LocalToWorld` computed, leaving world matrix calculated from old transform.
    registry.register_reflect_type::<LocalTransform>();
    registry.register_reflect_type::<TransformInterpolation2D>();

    // ── keyframe animation ──
    // Tracks contain nested structs, first register type (RTTIBin type table / editor field descriptions),
    // then register component itself.
    registry.register_reflect_type::<crate::animation::Vec3Key>();
    registry.register_reflect_type::<crate::animation::AnimationEvent>();
    registry.register_reflect_serde::<KeyframeAnimation>("prism_engine::scene::KeyframeAnimation");

    // ── particle emitter (author state; live particles in runtime `ParticleEmitter`) ──
    registry.register_reflect_serde::<ParticleSystem>("prism_engine::scene::ParticleSystem");
    registry.register_reflect_serde::<ParticleEmitter2D>("prism_engine::scene::ParticleEmitter2D");
    // Screen-space emitter: simulated in the engine, emitted as UI quads (#1057).
    registry.register_reflect_serde::<UiParticleEmitter>("prism_engine::scene::UiParticleEmitter");

    // ── audio (author state; decode and mixing in host layer `prism-app::scene_audio`) ──
    registry.register_reflect_serde::<AudioSource>("prism_engine::scene::AudioSource");
    registry.register_reflect_serde::<Terrain>("prism_engine::scene::Terrain");
    registry.register_reflect_type::<crate::voxel::VoxelBrick>();
    registry.register_reflect_serde::<crate::voxel::VoxelChunk>("prism_engine::scene::VoxelChunk");
    registry.register_reflect_serde::<crate::voxel::VoxelCursorHighlight>(
        "prism_engine::scene::VoxelCursorHighlight",
    );
    registry.register_reflect_serde::<PostProcessVolume>("prism_engine::scene::PostProcessVolume");
    // `Animator.layers` is nested struct; only after registering type can RSCN / editor schema parse.
    registry.register_reflect_type::<AnimLayer>();
    registry.register_reflect_serde::<Animator>("prism_engine::scene::Animator");
    registry.register_reflect_serde::<SkeletonRoot>("prism_engine::scene::SkeletonRoot");
    registry.register_reflect_serde::<FootIk>("prism_engine::scene::FootIk");
    registry
        .register_reflect_serde::<SkinnedMeshRenderer>("prism_engine::scene::SkinnedMeshRenderer");
    registry.register_reflect_serde::<Collider>("prism_engine::scene::Collider");
    #[cfg(feature = "group-2d")]
    registry.register_reflect_serde::<Collider2D>("prism_engine::scene::Collider2D");
    registry.register_reflect_serde::<Rigidbody>("prism_engine::scene::Rigidbody");
    registry
        .register_reflect_serde::<CharacterController>("prism_engine::scene::CharacterController");
    registry.register_reflect_serde::<AudioListener>("prism_engine::scene::AudioListener");

    // ── prefab instance (editor metadata) ──
    registry.register_reflect_serde::<PrefabInstance>("prism_engine::scene::PrefabInstance");

    // ── UI component  ──
    #[cfg(feature = "group-ui")]
    {
        registry.register_reflect_serde::<Layout>("prism_engine::ui::Layout");
        registry.register_reflect_serde::<Node>("prism_engine::ui::Node");
        registry.register_reflect_serde::<Style>("prism_engine::ui::Style");
        // `ComputedLayout` is output of layout system (`rect` rewritten every frame),
        // ensured by `ui::layout`; only register type, don't let author attach.
        registry.register_reflect_type::<ComputedLayout>();
        registry.register_reflect_serde::<Text>("prism_engine::ui::Text");
        // #528: components of interactive UI must be savable in scene / prefab.
        registry.register_reflect_serde::<Interaction>("prism_engine::ui::Interaction");
        registry.register_reflect_serde::<Button>("prism_engine::ui::Button");
        // #463: canvas scale (reference resolution → actual screen).
        registry.register_reflect_serde::<Canvas>("prism_engine::ui::Canvas");
    }

    // ── nested types (type table / cook needed) ──
    registry.register_reflect_type::<glam::Vec2>();
    registry.register_reflect_type::<glam::Vec3>();
    registry.register_reflect_type::<glam::Vec4>();
    registry.register_reflect_type::<glam::Quat>();
    #[cfg(feature = "group-ui")]
    {
        registry.register_reflect_type::<Anchors>();
        registry.register_reflect_type::<Pivot>();
        registry.register_reflect_type::<Margin>();
        // Nested type of `Style.layout` (#527 / #709): missing registration makes all scenes with `Style`
        // fail RSCN encode with `type ... is not registered`.
        registry.register_reflect_type::<Layout>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Camera, LocalTransform, Name};
    use prism_reflect::rtti_bin;

    fn test_registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry.register_reflect_type::<glam::Vec3>();
        registry.register_reflect_type::<glam::Quat>();
        registry.register_reflect::<Camera>();
        registry.register_reflect::<Name>();
        registry.register_reflect::<LocalTransform>();
        registry
    }

    #[test]
    fn reflected_components_roundtrip_through_rtti_bin() {
        let registry = test_registry();
        let camera = Camera {
            fov_y_degrees: 72.0,
            ..Camera::default()
        };
        let name = Name("player".into());
        let transform = LocalTransform {
            position: glam::Vec3::new(1.0, 2.0, 3.0),
            ..LocalTransform::default()
        };

        let objects = vec![camera.to_object(), name.to_object(), transform.to_object()];
        let bytes = rtti_bin::encode(&objects, registry.type_registry()).unwrap();
        let decoded = rtti_bin::decode(&bytes).unwrap();

        let mut world = World::new();
        let entity = world.spawn();
        for object in &decoded.objects {
            assert!(registry.apply_object(&mut world, entity, object));
        }

        assert_eq!(world.get::<Camera>(entity).unwrap().fov_y_degrees, 72.0);
        assert_eq!(world.get::<Name>(entity).unwrap().0, "player");
        assert_eq!(
            world.get::<LocalTransform>(entity).unwrap().position,
            glam::Vec3::new(1.0, 2.0, 3.0)
        );

        // export goes through same schema.
        let extracted = registry
            .extract_object_by_name(&world, entity, "prism_engine::scene::Camera")
            .unwrap();
        assert_eq!(extracted, camera.to_object());
    }

    #[test]
    fn unregistered_object_is_rejected() {
        let registry = ComponentRegistry::new();
        let object = Camera::default().to_object();
        let mut world = World::new();
        let entity = world.spawn();
        assert!(!registry.apply_object(&mut world, entity, &object));
        assert!(world.get::<Camera>(entity).is_none());
    }

    #[test]
    fn builtin_registry_is_shared() {
        // Project modules and the engine must hash the same schema, so the
        // registry is built once and handed out by reference.
        assert!(std::ptr::eq(builtin_registry(), builtin_registry()));
        assert_eq!(
            builtin_registry().schema_fingerprint(),
            builtin_schema_fingerprint()
        );
    }

    #[test]
    fn builtin_components_expose_defaults_and_names() {
        let mut registry = ComponentRegistry::new();
        crate::registry::register_builtin_components(&mut registry);

        // Each built-in component has "add component" default value, and can deserialize back to component.
        let names = registry.component_names();

        // schema_fingerprint is deterministic and non-zero
        let fp1 = registry.schema_fingerprint();
        assert_ne!(fp1, 0);
        let fp2 = builtin_schema_fingerprint();
        assert_eq!(fp1, fp2);
        assert!(names.contains(&"prism_engine::scene::Camera"));
        for name in &names {
            let default = registry
                .default_json(name)
                .unwrap_or_else(|| panic!("missing default for {name}"));
            let mut world = World::new();
            let entity = world.spawn();
            registry.apply(&mut world, entity, name, &default);

            // Export via reflection path, shape should match default (Name uses custom converter).
            if let Some(object) = registry.extract_object_by_name(&world, entity, name) {
                assert_eq!(
                    registry.object_from_json(name, &default),
                    Some(object),
                    "{name} default JSON does not round-trip"
                );
            }
        }

        // Name default shape is bare string (custom JSON shape).
        assert_eq!(
            registry.default_json("prism_engine::scene::Name"),
            Some(serde_json::json!("entity"))
        );

        // Types nested in author components must also be registered, otherwise RSCN encode will
        // fail with `type ... is not registered` (#709 `Style.layout`).
        for nested in [
            "prism_engine::ui::Anchors",
            "prism_engine::ui::Pivot",
            "prism_engine::ui::Margin",
            "prism_engine::ui::Layout",
        ] {
            assert!(
                registry
                    .type_registry()
                    .descriptor(TypeId::from_name(nested))
                    .is_some(),
                "nested type {nested} is not registered"
            );
        }

        // Each component can round-trip "author JSON -> reflection object -> author JSON".
        for name in &names {
            let default = registry.default_json(name).unwrap();
            let object = registry
                .object_from_json(name, &default)
                .unwrap_or_else(|| panic!("{name} default JSON is not convertable"));
            let back = registry
                .object_to_json(name, &object)
                .unwrap_or_else(|| panic!("{name} has no object_to_json"));
            assert_eq!(back, default, "{name} JSON/reflect round-trip mismatch");
        }
    }
}
