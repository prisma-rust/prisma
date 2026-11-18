use std::collections::HashMap;

use prism_ecs::{Entity, World};
use proptest::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag(i32);

#[derive(Debug, Clone)]
enum Op {
    Spawn(Option<i32>),
    Despawn(usize),
    SetTag(usize, i32),
    RemoveTag(usize),
    SetActive(usize, bool),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        prop::option::of(-100i32..100i32).prop_map(Op::Spawn),
        (0usize..64).prop_map(Op::Despawn),
        (0usize..64, -100i32..100i32).prop_map(|(i, v)| Op::SetTag(i, v)),
        (0usize..64).prop_map(Op::RemoveTag),
        (0usize..64, any::<bool>()).prop_map(|(i, b)| Op::SetActive(i, b)),
    ]
}

proptest! {
    #[test]
    fn world_matches_reference_model(ops in prop::collection::vec(op_strategy(), 1..60)) {
        let mut world = World::new();
        let mut live: Vec<Entity> = Vec::new();
        let mut retired: Vec<Entity> = Vec::new();
        let mut tags: HashMap<Entity, i32> = HashMap::new();
        let mut active: HashMap<Entity, bool> = HashMap::new();

        for op in ops {
            match op {
                Op::Spawn(maybe_tag) => {
                    let e = world.spawn();
                    live.push(e);
                    active.insert(e, true);
                    if let Some(v) = maybe_tag {
                        world.insert(e, Tag(v));
                        tags.insert(e, v);
                    }
                }
                Op::Despawn(idx) if !live.is_empty() => {
                    let e = live.swap_remove(idx % live.len());
                    world.despawn(e);
                    retired.push(e);
                    tags.remove(&e);
                    active.remove(&e);
                }
                Op::SetTag(idx, v) if !live.is_empty() => {
                    let e = live[idx % live.len()];
                    world.insert(e, Tag(v));
                    tags.insert(e, v);
                }
                Op::RemoveTag(idx) if !live.is_empty() => {
                    let e = live[idx % live.len()];
                    let removed = world.remove::<Tag>(e);
                    prop_assert_eq!(removed.map(|t| t.0), tags.remove(&e));
                }
                Op::SetActive(idx, flag) if !live.is_empty() => {
                    let e = live[idx % live.len()];
                    world.set_active(e, flag);
                    active.insert(e, flag);
                }
                _ => {}
            }
        }

        for &e in &live {
            prop_assert!(world.is_alive(e));
            prop_assert_eq!(world.is_active(e), active[&e]);
            prop_assert_eq!(world.get::<Tag>(e).map(|t| t.0), tags.get(&e).copied());
        }
        for &e in &retired {
            prop_assert!(!world.is_alive(e));
            prop_assert_eq!(world.get::<Tag>(e), None);
        }

        let active_queried: HashMap<Entity, i32> = world.query::<Tag>().map(|(e, t)| (e, t.0)).collect();
        let expected_active: HashMap<Entity, i32> = tags
            .iter()
            .filter(|(e, _)| active.get(e).copied().unwrap_or(false))
            .map(|(e, v)| (*e, *v))
            .collect();
        prop_assert_eq!(active_queried, expected_active);

        let all_queried: HashMap<Entity, i32> = world
            .query_inactive_inclusive::<Tag>()
            .map(|(e, t)| (e, t.0))
            .collect();
        prop_assert_eq!(all_queried, tags);
    }
}
