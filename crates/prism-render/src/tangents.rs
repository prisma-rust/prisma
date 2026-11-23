//! Derive vertex tangents from UV (#515).
//!
//! Import mesh intermediate format has no tangent slot, `scene/loader.rs` only fills constant
//! `[1, 0, 0, 1]` per vertex; fragment shader actually uses this tangent to build TBN, so all imported mesh normal
//! texture directions are wrong (faces with normal ±X even degenerate).
//!
//! Here **before upload** accumulate tangents per triangle by UV, the basic form of MikkTSpace:
//!
//! ```text
//! e1 = p1 - p0              duv1 = uv1 - uv0
//! e2 = p2 - p0              duv2 = uv2 - uv0
//! r  = 1 / (duv1.x * duv2.y - duv2.x * duv1.y)
//! T  = (e1 * duv2.y - e2 * duv1.y) * r
//! B  = (e2 * duv1.x - e1 * duv2.x) * r
//! ```
//!
//! Each vertex then Gram-Schmidt orthogonalizes `T` against normal (removes normal direction component), and uses
//! `w = sign(dot(cross(N, T), B))` to record handedness — consistent with `scene_frag.slang` convention
//! `b = cross(N, T) * w`.
//!
//! Only used when **caller has no tangent available** (fully constant / insufficient length), see
//! `managers::mesh_manager::build_vertices`. Pure math, no dependency, does not pull in `glam` like `culling`.
//! does not pull in `glam` like `culling`.

/// Tangent / bitangent accumulation result for a group of triangles, internal use.
struct Accum {
    tangent: [f32; 3],
    bitangent: [f32; 3],
}

const FALLBACK: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let length = dot(a, a).sqrt();
    if !length.is_finite() || length < 1e-8 {
        return None;
    }
    Some(scale(a, 1.0 / length))
}

/// Generate tangents per vertex by UV, `w` in `+1` / `-1`.
///
/// Length equal to `positions.len()`; returns `None` when input lengths mismatch or no triangles.
/// Degenerate UV (triangle UV area 0) not accumulated, when whole mesh degenerate vertex falls back to
/// `[1, 0, 0, 1]`, caller decides whether to log.
pub fn generate(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Option<Vec<[f32; 4]>> {
    if positions.len() < 3
        || normals.len() != positions.len()
        || uvs.len() != positions.len()
        || indices.len() < 3
    {
        return None;
    }

    let mut accum: Vec<Accum> = (0..positions.len())
        .map(|_| Accum {
            tangent: [0.0; 3],
            bitangent: [0.0; 3],
        })
        .collect();

    for triangle in indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        if i0 >= positions.len() || i1 >= positions.len() || i2 >= positions.len() {
            continue;
        }
        let (p0, p1, p2) = (positions[i0], positions[i1], positions[i2]);
        let (uv0, uv1, uv2) = (uvs[i0], uvs[i1], uvs[i2]);

        let e1 = sub(p1, p0);
        let e2 = sub(p2, p0);
        let duv1 = [uv1[0] - uv0[0], uv1[1] - uv0[1]];
        let duv2 = [uv2[0] - uv0[0], uv2[1] - uv0[1]];

        let determinant = duv1[0] * duv2[1] - duv2[0] * duv1[1];
        if !determinant.is_finite() || determinant.abs() < 1e-10 {
            continue;
        }
        let r = 1.0 / determinant;
        let tangent = scale(sub(scale(e1, duv2[1]), scale(e2, duv1[1])), r);
        let bitangent = scale(sub(scale(e2, duv1[0]), scale(e1, duv2[0])), r);

        for index in [i0, i1, i2] {
            accum[index].tangent = add(accum[index].tangent, tangent);
            accum[index].bitangent = add(accum[index].bitangent, bitangent);
        }
    }

    let mut out = Vec::with_capacity(positions.len());
    for (index, entry) in accum.into_iter().enumerate() {
        let normal = normalize(normals[index]).unwrap_or([0.0, 0.0, 1.0]);
        let projected = sub(entry.tangent, scale(normal, dot(normal, entry.tangent)));
        let Some(tangent) = normalize(projected) else {
            out.push(FALLBACK);
            continue;
        };
        let handedness = if dot(cross(normal, tangent), entry.bitangent) < 0.0 {
            -1.0
        } else {
            1.0
        };
        out.push([tangent[0], tangent[1], tangent[2], handedness]);
    }
    Some(out)
}

/// Whether given tangents are usable: count matches and not a constant.
///
/// Import path writes `[1, 0, 0, 1]` per vertex, so before render must detect this case —
/// real mesh cannot have all vertex tangents same direction (sphere, cube, any curved model differ).
pub fn are_placeholders(tangents: &[[f32; 4]], vertex_count: usize) -> bool {
    if tangents.len() != vertex_count || tangents.is_empty() {
        return true;
    }
    tangents
        .iter()
        .all(|tangent| tangent == &tangents[0] && tangent[3].abs() == 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vertex triple + index, passed in test in batch.
    struct Quad {
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
    }

    /// Identity square on XY plane, normal +Z; `uvs` decides which direction tangent should point.
    fn quad(uvs: [[f32; 2]; 4]) -> Quad {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let normals = vec![[0.0, 0.0, 1.0]; 4];
        let indices = vec![0, 1, 2, 0, 2, 3];
        Quad {
            positions,
            normals,
            uvs: uvs.to_vec(),
            indices,
        }
    }

    #[test]
    fn tangents_follow_the_uvs_not_the_x_axis() {
        let uvs = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");

        for tangent in &tangents {
            assert!(
                (tangent[0]).abs() < 1e-5 && (tangent[1] - 1.0).abs() < 1e-5,
                "expected +Y tangent, got {tangent:?}"
            );
        }
    }

    #[test]
    fn a_plain_uv_quad_gets_plus_x_tangents() {
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");
        for tangent in &tangents {
            assert!((tangent[0] - 1.0).abs() < 1e-5, "got {tangent:?}");
            assert!((tangent[3] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn mirrored_uvs_flip_the_handedness() {
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");
        for tangent in &tangents {
            assert!((tangent[0] - 1.0).abs() < 1e-5, "got {tangent:?}");
            assert!(
                (tangent[3] + 1.0).abs() < 1e-5,
                "expected w = -1, got {tangent:?}"
            );
        }
    }

    #[test]
    fn degenerate_uvs_fall_back_to_the_x_axis() {
        let uvs = [[0.0, 0.0]; 4];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");
        assert!(tangents.iter().all(|t| *t == FALLBACK));
    }

    #[test]
    fn mismatched_inputs_are_rejected() {
        let positions = vec![[0.0, 0.0, 0.0]; 3];
        let normals = vec![[0.0, 0.0, 1.0]; 2];
        let uvs = vec![[0.0, 0.0]; 3];
        assert!(generate(&positions, &normals, &uvs, &[0, 1, 2]).is_none());
        assert!(generate(&positions, &normals, &uvs, &[]).is_none());
    }

    #[test]
    fn placeholders_are_detected_by_uniformity() {
        assert!(are_placeholders(&[[1.0, 0.0, 0.0, 1.0]; 4], 4));
        assert!(
            are_placeholders(&[], 4),
            "Empty tangent should also count as placeholder"
        );
        assert!(!are_placeholders(
            &[[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]],
            2
        ));
    }
}
