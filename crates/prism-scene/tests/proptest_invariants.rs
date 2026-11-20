use std::collections::HashMap;

use prism_scene::json::{EntityJson, SceneJson, TransformJson};
use prism_scene::registry::builtin_registry;
use prism_scene::rscn;
use proptest::prelude::*;

fn finite_coord() -> impl Strategy<Value = f32> {
    -1.0e4f32..1.0e4f32
}

fn transform_strategy() -> impl Strategy<Value = TransformJson> {
    (
        (finite_coord(), finite_coord(), finite_coord()),
        (
            -1.0f32..1.0f32,
            -1.0f32..1.0f32,
            -1.0f32..1.0f32,
            -1.0f32..1.0f32,
        ),
        (0.01f32..100.0f32, 0.01f32..100.0f32, 0.01f32..100.0f32),
    )
        .prop_map(
            |((px, py, pz), (rx, ry, rz, rw), (sx, sy, sz))| TransformJson {
                position: [px, py, pz],
                rotation: [rx, ry, rz, rw],
                scale: [sx, sy, sz],
            },
        )
}

proptest! {
    #[test]
    fn rscn_v4_roundtrip_preserves_entities_and_hierarchy(
        specs in prop::collection::vec(
            ("[a-zA-Z0-9_-]{1,20}", transform_strategy(), any::<bool>(), any::<bool>()),
            1..12,
        )
    ) {
        let registry = builtin_registry();
        let mut entities = Vec::with_capacity(specs.len());
        for (idx, (name, transform, has_parent, active)) in specs.into_iter().enumerate() {
            let id = (idx as u32) + 1;
            let parent = if idx > 0 && has_parent {
                Some(idx as u32)
            } else {
                None
            };
            let mut components = HashMap::new();
            components.insert(
                "prism_engine::scene::Active".to_string(),
                serde_json::json!(active),
            );
            entities.push(EntityJson {
                id,
                name: Some(name),
                parent,
                transform,
                components,
            });
        }

        let scene = SceneJson {
            version: 3,
            entities,
        };
        let bytes = rscn::encode_scene(&scene, registry).expect("encode RSCN v4");
        prop_assert!(rscn::is_v4(&bytes));

        let decoded = rscn::decode_scene(&bytes, registry).expect("decode RSCN v4");
        let order = scene.topological_order();
        let mut pos_by_id = HashMap::new();
        for (pos, &orig_idx) in order.iter().enumerate() {
            pos_by_id.insert(scene.entities[orig_idx].id, pos as u32);
        }

        prop_assert_eq!(decoded.entities.len(), scene.entities.len());
        for (dec, &orig_idx) in decoded.entities.iter().zip(order.iter()) {
            let orig = &scene.entities[orig_idx];
            prop_assert_eq!(dec.name.as_str(), orig.name.as_deref().unwrap_or(""));
            prop_assert_eq!(dec.position, orig.transform.position);
            prop_assert_eq!(dec.rotation, orig.transform.rotation);
            prop_assert_eq!(dec.scale, orig.transform.scale);
            let expected_parent = orig.parent.map(|pid| pos_by_id[&pid]);
            prop_assert_eq!(dec.parent, expected_parent);
        }
    }

    #[test]
    fn rscn_decode_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = rscn::decode_scene(&bytes, builtin_registry());
    }

    #[test]
    fn probe_volume_roundtrips_through_rscn_v4(
        mode_idx in 0u8..3u8,
        relocation in any::<bool>(),
        classification in any::<bool>(),
        scrolling in any::<bool>(),
        variability in any::<bool>(),
        spacing in 0.5f32..8.0f32,
        rays in 32u32..256u32,
    ) {
        let mode = match mode_idx {
            0 => "Off",
            1 => "Baked",
            _ => "Dynamic",
        };
        let mut components = HashMap::new();
        components.insert(
            "prism_engine::scene::ProbeVolume".to_string(),
            serde_json::json!({
                "mode": mode,
                "origin": [1.0, 2.0, -3.0],
                "spacing": [spacing, spacing, spacing],
                "probe_counts": [8, 4, 8],
                "rays_per_probe": rays,
                "hysteresis": 0.95,
                "relocation": relocation,
                "classification": classification,
                "scrolling": scrolling,
                "variability": variability,
                "baked_volume": null
            }),
        );
        let scene = SceneJson {
            version: 3,
            entities: vec![EntityJson {
                id: 1,
                name: Some("ProbeVolume".to_string()),
                parent: None,
                transform: TransformJson::default(),
                components,
            }],
        };
        let registry = builtin_registry();
        let bytes = rscn::encode_scene(&scene, registry).expect("encode ProbeVolume RSCN v4");
        let decoded = rscn::decode_scene(&bytes, registry).expect("decode ProbeVolume RSCN v4");
        prop_assert_eq!(decoded.entities.len(), 1);
        prop_assert_eq!(decoded.entities[0].components.len(), 1);
    }
}
