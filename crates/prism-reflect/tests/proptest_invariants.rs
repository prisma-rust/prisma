use glam::Vec3;
use prism_reflect::{rtti_bin, Reflect, TypeRegistry};
use proptest::prelude::*;

#[derive(Debug, Clone, Default, PartialEq, Reflect)]
#[reflect(name = "test::PropSample")]
struct PropSample {
    flag: bool,
    count: i32,
    ratio: f32,
    label: String,
    tags: Vec<i32>,
    note: Option<String>,
    offset: Vec3,
}

fn finite_f32() -> impl Strategy<Value = f32> {
    -1.0e4f32..1.0e4f32
}

fn prop_sample_strategy() -> impl Strategy<Value = PropSample> {
    (
        any::<bool>(),
        any::<i32>(),
        finite_f32(),
        ".{0,32}",
        prop::collection::vec(any::<i32>(), 0..16),
        prop::option::of(".{0,24}"),
        (finite_f32(), finite_f32(), finite_f32()),
    )
        .prop_map(
            |(flag, count, ratio, label, tags, note, (x, y, z))| PropSample {
                flag,
                count,
                ratio,
                label,
                tags,
                note,
                offset: Vec3::new(x, y, z),
            },
        )
}

proptest! {
    #[test]
    fn rtti_bin_roundtrip_preserves_objects(samples in prop::collection::vec(prop_sample_strategy(), 0..8)) {
        let mut registry = TypeRegistry::new();
        registry.register::<Vec3>();
        registry.register::<PropSample>();

        let objects: Vec<_> = samples.iter().map(Reflect::to_object).collect();
        let bytes = rtti_bin::encode(&objects, &registry).expect("encode RTTIBin");
        let decoded = rtti_bin::decode(&bytes).expect("decode RTTIBin");

        prop_assert_eq!(decoded.objects.len(), samples.len());
        for (orig, obj) in samples.iter().zip(decoded.objects.iter()) {
            let boxed = registry
                .from_object(obj)
                .expect("restore PropSample from RTTIBin");
            let restored = boxed.downcast::<PropSample>().expect("downcast PropSample");
            prop_assert_eq!(restored.as_ref(), orig);
        }
    }

    #[test]
    fn rtti_bin_decode_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = rtti_bin::decode(&bytes);
    }
}
