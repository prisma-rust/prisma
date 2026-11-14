use super::*;

#[test]
fn frame_ubo_data_size_is_432() {
    // std140: legacy fields end at 272, the next mat4 aligns to 288, then the
    // CSM tail (shadowSplit @ 352, cascade count @ 356, pad, previous VP @ 368)
    // matches the Slang `FrameUBO` mirror in common.slang. These are the offsets
    // the committed reflection JSON in assets/shaders/reflection reports.
    assert_eq!(std::mem::size_of::<FrameUBOData>(), 432);
}

#[test]
fn frame_ubo_data_matches_the_committed_shader_layout() {
    // The GPU is handed `FrameUBOData` verbatim and the fragment shader reads it
    // through std140 offsets taken from `common.slang`. Nothing else links the
    // two: a mismatch silently feeds the shader the wrong bytes (which is how
    // `shadowCascadeCount` once ended up reading `prevViewProj`). Compare the
    // Rust layout against the committed reflection JSON so a drift fails here
    // instead of as a rendering bug.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/shaders/reflection/scene_frag.json"
    );
    let text = std::fs::read_to_string(path).expect("committed scene_frag reflection JSON");
    let json: serde_json::Value = serde_json::from_str(&text).expect("reflection JSON parses");

    // The reflection repeats the type once per entry point; any copy is the same
    // layout, so walk until the first struct literally named `FrameUBO`.
    fn find_ubo(node: &serde_json::Value) -> Option<&serde_json::Value> {
        match node {
            serde_json::Value::Object(map) => {
                if map.get("name").and_then(|n| n.as_str()) == Some("FrameUBO")
                    && map.contains_key("fields")
                {
                    return Some(node);
                }
                map.values().find_map(find_ubo)
            }
            serde_json::Value::Array(items) => items.iter().find_map(find_ubo),
            _ => None,
        }
    }
    let ubo = find_ubo(&json).expect("FrameUBO struct in the reflection JSON");

    let reflected = |name: &str| -> u64 {
        ubo["fields"]
            .as_array()
            .expect("fields array")
            .iter()
            .find(|f| f["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("field {name} in the reflected FrameUBO"))["binding"]
            ["offset"]
            .as_u64()
            .expect("integer offset")
    };
    let size = ubo["sizes"][0]["value"].as_u64().expect("struct size");

    assert_eq!(
        reflected("viewProj") as usize,
        std::mem::offset_of!(FrameUBOData, view_proj)
    );
    assert_eq!(
        reflected("lightViewProj") as usize,
        std::mem::offset_of!(FrameUBOData, light_view_proj)
    );
    assert_eq!(
        reflected("lightViewProj1") as usize,
        std::mem::offset_of!(FrameUBOData, light_view_proj1)
    );
    assert_eq!(
        reflected("shadowSplit") as usize,
        std::mem::offset_of!(FrameUBOData, shadow_split)
    );
    assert_eq!(
        reflected("shadowCascadeCount") as usize,
        std::mem::offset_of!(FrameUBOData, shadow_cascade_count)
    );
    assert_eq!(
        reflected("prevViewProj") as usize,
        std::mem::offset_of!(FrameUBOData, prev_view_proj)
    );
    assert_eq!(size as usize, std::mem::size_of::<FrameUBOData>());
}

#[test]
fn gpu_light_size_is_64() {
    // P3b：点光 + 聚光共用一条 SSBO，多了 direction / params 两个 float4。
    assert_eq!(std::mem::size_of::<GpuLight>(), 64);
    assert_eq!(GPU_LIGHT_SIZE, 64);
}

#[test]
fn gpu_light_offsets() {
    assert_eq!(std::mem::offset_of!(GpuLight, position), 0);
    assert_eq!(std::mem::offset_of!(GpuLight, color), 16);
    assert_eq!(std::mem::offset_of!(GpuLight, direction), 32);
    assert_eq!(std::mem::offset_of!(GpuLight, params), 48);
}

#[test]
fn frame_ubo_data_offsets() {
    assert_eq!(std::mem::offset_of!(FrameUBOData, view_proj), 0);
    assert_eq!(std::mem::offset_of!(FrameUBOData, camera_position), 64);
    assert_eq!(std::mem::offset_of!(FrameUBOData, light_direction), 80);
    assert_eq!(std::mem::offset_of!(FrameUBOData, light_color), 96);
    assert_eq!(std::mem::offset_of!(FrameUBOData, view), 112);
    assert_eq!(std::mem::offset_of!(FrameUBOData, light_view_proj), 176);
    assert_eq!(std::mem::offset_of!(FrameUBOData, tonemap_mode), 240);
    assert_eq!(std::mem::offset_of!(FrameUBOData, viewport_size), 244);
    assert_eq!(std::mem::offset_of!(FrameUBOData, exposure), 252);
    assert_eq!(std::mem::offset_of!(FrameUBOData, shadow_strength), 256);
    assert_eq!(std::mem::offset_of!(FrameUBOData, _pad2), 260);
    assert_eq!(std::mem::offset_of!(FrameUBOData, _pad2b), 272);
    assert_eq!(std::mem::offset_of!(FrameUBOData, light_view_proj1), 288);
    assert_eq!(std::mem::offset_of!(FrameUBOData, shadow_split), 352);
    assert_eq!(
        std::mem::offset_of!(FrameUBOData, shadow_cascade_count),
        356
    );
    assert_eq!(std::mem::offset_of!(FrameUBOData, _pad3), 360);
    assert_eq!(std::mem::offset_of!(FrameUBOData, prev_view_proj), 368);
}
