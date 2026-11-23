//! Procedural primitive meshes (equivalent of `GameObject → 3D Object`).
//!
//! Editor presets reference these via `mesh_path = "__builtin/<name>"`;
//! `prism-engine` asset resolver generates `MeshUploadInput` directly when it sees this prefix,
//! no `.pak` read, no cook needed — consistent with engine built-in Cube/Sphere/Capsule.
//!
//! Identity convention (consistent with engine coordinates: right-handed, +Y up):
//! - box / sphere / cylinder / capsule: outer size ≈ 1 (radius 0.5, height 1), centered at origin;
//! - plane / quad: 1×1, on XZ plane at `y = 0`, normal +Y.
//!
//! All meshes have position / normal / UV / tangent / index, color left empty (renderer falls back to white).

use crate::managers::MeshUploadInput;

/// Registry name → mesh. `path` can be `__builtin/cube` or bare `cube`.
pub fn builtin(path: &str) -> Option<MeshUploadInput> {
    let name = path.strip_prefix("__builtin/").unwrap_or(path);
    Some(match name {
        "box" | "cube" => box_mesh(),
        "sphere" => sphere_mesh(0.5, 16, 24),
        "cylinder" => cylinder_mesh(0.5, 1.0, 24),
        "capsule" => capsule_mesh(0.5, 1.0, 24, 8),
        "quad" | "plane" => quad_mesh(),
        _ => return None,
    })
}

/// This path is not a built-in mesh (asset resolver uses it to short-circuit `.pak` lookup).
pub fn is_builtin(path: &str) -> bool {
    path.starts_with("__builtin/")
}

/// All built-in names (for docs / menus).
pub const BUILTIN_NAMES: &[&str] = &["box", "sphere", "cylinder", "capsule", "quad"];

fn push_vertex(
    out: &mut MeshUploadInput,
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    tangent: [f32; 3],
) -> u32 {
    let index = out.positions.len() as u32;
    out.positions.push(position);
    out.normals.push(normal);
    out.uvs.push(uv);
    out.tangents.push([tangent[0], tangent[1], tangent[2], 1.0]);
    index
}

fn empty_mesh() -> MeshUploadInput {
    MeshUploadInput {
        positions: Vec::new(),
        normals: Vec::new(),
        colors: Vec::new(),
        uvs: Vec::new(),
        tangents: Vec::new(),
        indices: Vec::new(),
    }
}

/// One face of cube: (normal, tangent, in-face u axis, in-face v axis).
type BoxFace = ([f32; 3], [f32; 3], [f32; 3], [f32; 3]);

/// Identity cube (6 faces each with independent vertices, normal / UV matching face direction).
pub fn box_mesh() -> MeshUploadInput {
    let mut mesh = empty_mesh();
    // (normal, tangent, in-face u axis, in-face v axis)
    let faces: [BoxFace; 6] = [
        (
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
        ),
        (
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
        ),
        (
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ),
        (
            [0.0, 0.0, -1.0],
            [-1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ),
        (
            [1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [0.0, 1.0, 0.0],
        ),
        (
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
        ),
    ];
    for (normal, tangent, u_axis, v_axis) in faces {
        let base = mesh.positions.len() as u32;
        for (u, v) in [(0.0_f32, 0.0_f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let cu = u - 0.5;
            let cv = v - 0.5;
            let half = 0.5;
            let position = [
                normal[0] * half + u_axis[0] * cu + v_axis[0] * cv,
                normal[1] * half + u_axis[1] * cu + v_axis[1] * cv,
                normal[2] * half + u_axis[2] * cu + v_axis[2] * cv,
            ];
            push_vertex(&mut mesh, position, normal, [u, 1.0 - v], tangent);
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh
}

/// UV sphere (longitude × latitude), poles use fan triangles.
pub fn sphere_mesh(radius: f32, rings: usize, segments: usize) -> MeshUploadInput {
    let mut mesh = empty_mesh();
    let rings = rings.max(2);
    let segments = segments.max(3);
    for ring in 0..=rings {
        let v = ring as f32 / rings as f32;
        let phi = v * std::f32::consts::PI;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let normal = [sin_phi * cos_theta, cos_phi, sin_phi * sin_theta];
            let position = [normal[0] * radius, normal[1] * radius, normal[2] * radius];
            let tangent = [-sin_theta, 0.0, cos_theta];
            push_vertex(&mut mesh, position, normal, [u, 1.0 - v], tangent);
        }
    }
    let stride = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = (ring * stride + segment) as u32;
            let b = a + stride as u32;
            let c = b + 1;
            let d = a + 1;
            // Skip degenerate triangles (poles).
            if ring != 0 {
                mesh.indices.extend_from_slice(&[a, d, b]);
            }
            if ring != rings - 1 {
                mesh.indices.extend_from_slice(&[d, c, b]);
            }
        }
    }
    mesh
}

/// Cylinder (side + top and bottom caps), height `height`, radius `radius`, centered at origin.
pub fn cylinder_mesh(radius: f32, height: f32, segments: usize) -> MeshUploadInput {
    let mut mesh = empty_mesh();
    let segments = segments.max(3);
    let half = height * 0.5;

    for segment in 0..=segments {
        let u = segment as f32 / segments as f32;
        let theta = u * std::f32::consts::TAU;
        let (sin_theta, cos_theta) = theta.sin_cos();
        let normal = [cos_theta, 0.0, sin_theta];
        let tangent = [-sin_theta, 0.0, cos_theta];
        push_vertex(
            &mut mesh,
            [normal[0] * radius, -half, normal[2] * radius],
            normal,
            [u, 1.0],
            tangent,
        );
        push_vertex(
            &mut mesh,
            [normal[0] * radius, half, normal[2] * radius],
            normal,
            [u, 0.0],
            tangent,
        );
    }
    for segment in 0..segments {
        let a = (segment * 2) as u32;
        let b = a + 1;
        let c = a + 2;
        let d = a + 3;
        mesh.indices.extend_from_slice(&[a, c, b, b, c, d]);
    }

    // Top / bottom caps (center point + fan), normals ±Y.
    for (y, ny, flip) in [(half, 1.0_f32, false), (-half, -1.0_f32, true)] {
        let center = push_vertex(
            &mut mesh,
            [0.0, y, 0.0],
            [0.0, ny, 0.0],
            [0.5, 0.5],
            [1.0, 0.0, 0.0],
        );
        let base = mesh.positions.len() as u32;
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let (sin_theta, cos_theta) = theta.sin_cos();
            push_vertex(
                &mut mesh,
                [cos_theta * radius, y, sin_theta * radius],
                [0.0, ny, 0.0],
                [cos_theta * 0.5 + 0.5, sin_theta * 0.5 + 0.5],
                [1.0, 0.0, 0.0],
            );
        }
        for segment in 0..segments {
            let a = base + segment as u32;
            let b = a + 1;
            if flip {
                mesh.indices.extend_from_slice(&[center, b, a]);
            } else {
                mesh.indices.extend_from_slice(&[center, a, b]);
            }
        }
    }
    mesh
}

/// Capsule: intermediate cylinder (height `height`) + top and bottom hemispheres (radius `radius`), centered at origin.
pub fn capsule_mesh(
    radius: f32,
    height: f32,
    segments: usize,
    cap_rings: usize,
) -> MeshUploadInput {
    let mut mesh = empty_mesh();
    let segments = segments.max(3);
    let cap_rings = cap_rings.max(2);
    let half = height * 0.5;

    // Top hemisphere (phi: 0 → π/2, from +Y pole down to equator).
    for ring in 0..=cap_rings {
        let v = ring as f32 / cap_rings as f32;
        let phi = v * std::f32::consts::FRAC_PI_2;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let normal = [sin_phi * cos_theta, cos_phi, sin_phi * sin_theta];
            push_vertex(
                &mut mesh,
                [
                    normal[0] * radius,
                    half + normal[1] * radius,
                    normal[2] * radius,
                ],
                normal,
                [u, 1.0 - v * 0.5],
                [-sin_theta, 0.0, cos_theta],
            );
        }
    }
    // Bottom hemisphere (phi: π/2 → π).
    for ring in 0..=cap_rings {
        let v = ring as f32 / cap_rings as f32;
        let phi = std::f32::consts::FRAC_PI_2 + v * std::f32::consts::FRAC_PI_2;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let normal = [sin_phi * cos_theta, cos_phi, sin_phi * sin_theta];
            push_vertex(
                &mut mesh,
                [
                    normal[0] * radius,
                    -half + normal[1] * radius,
                    normal[2] * radius,
                ],
                normal,
                [u, 1.0 - v * 0.5 - 0.5],
                [-sin_theta, 0.0, cos_theta],
            );
        }
    }

    // Top + bottom hemispheres each connect to themselves (equatorial ring vertices coincide, normals differ,
    // same as sphere, just split into two fan triangle segments).
    let stride = segments + 1;
    let top_rings = cap_rings;
    let total_rings = top_rings + cap_rings + 1;
    for ring in 0..total_rings {
        for segment in 0..segments {
            let a = (ring * stride + segment) as u32;
            let b = a + stride as u32;
            let c = b + 1;
            let d = a + 1;
            if !(ring == 0 || ring == total_rings - 1) {
                mesh.indices.extend_from_slice(&[a, d, b, d, c, b]);
            }
        }
    }
    mesh
}

/// 1×1 XZ plane (normal +Y), Quad / Plane shared.
pub fn quad_mesh() -> MeshUploadInput {
    let mut mesh = empty_mesh();
    let corners = [
        ([-0.5, 0.0, 0.5], [0.0, 0.0]),
        ([0.5, 0.0, 0.5], [1.0, 0.0]),
        ([0.5, 0.0, -0.5], [1.0, 1.0]),
        ([-0.5, 0.0, -0.5], [0.0, 1.0]),
    ];
    for (position, uv) in corners {
        push_vertex(&mut mesh, position, [0.0, 1.0, 0.0], uv, [1.0, 0.0, 0.0]);
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(mesh: &MeshUploadInput) {
        assert!(!mesh.positions.is_empty(), "mesh has no vertices");
        assert!(!mesh.indices.is_empty(), "mesh has no indices");
        assert_eq!(mesh.positions.len(), mesh.normals.len());
        assert_eq!(mesh.positions.len(), mesh.uvs.len());
        assert_eq!(mesh.positions.len(), mesh.tangents.len());
        assert!(
            mesh.indices.len().is_multiple_of(3),
            "index count not a multiple of 3"
        );
        let max = mesh.positions.len() as u32;
        assert!(
            mesh.indices.iter().all(|index| *index < max),
            "index out of range"
        );
        for normal in &mesh.normals {
            let length =
                (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            assert!(
                (length - 1.0).abs() < 1e-3,
                "normal not unit length: {length}"
            );
        }
        for position in &mesh.positions {
            assert!(position.iter().all(|value| value.is_finite()));
            // Capsule = cylinder 1 + two radius 0.5 hemispheres → total height 2 (consistent with engine default).
            assert!(
                position.iter().all(|value| value.abs() <= 1.05),
                "primitive should be unit sized: {position:?}"
            );
        }
    }

    #[test]
    fn every_builtin_name_resolves_and_is_valid() {
        for name in BUILTIN_NAMES {
            let mesh = builtin(&format!("__builtin/{name}")).expect(name);
            check(&mesh);
        }
        // Aliases: cube → box, plane → quad.
        assert!(builtin("__builtin/cube").is_some());
        assert!(builtin("__builtin/plane").is_some());
        assert!(builtin("sphere").is_some());
        assert!(builtin("__builtin/nope").is_none());
    }

    #[test]
    fn is_builtin_matches_prefix() {
        assert!(is_builtin("__builtin/box"));
        assert!(!is_builtin("assets/meshes/cube.mesh"));
    }

    #[test]
    fn box_has_six_quads() {
        let mesh = box_mesh();
        assert_eq!(mesh.positions.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn quad_is_unit_square_on_ground() {
        let mesh = quad_mesh();
        assert_eq!(mesh.positions.len(), 4);
        assert!(mesh.positions.iter().all(|position| position[1] == 0.0));
        assert!(mesh.normals.iter().all(|normal| normal[1] == 1.0));
    }

    #[test]
    fn capsule_spans_expected_height() {
        let mesh = capsule_mesh(0.5, 1.0, 12, 4);
        let min = mesh
            .positions
            .iter()
            .map(|position| position[1])
            .fold(f32::MAX, f32::min);
        let max = mesh
            .positions
            .iter()
            .map(|position| position[1])
            .fold(f32::MIN, f32::max);
        assert!((min + 1.0).abs() < 1e-3, "capsule bottom {min}");
        assert!((max - 1.0).abs() < 1e-3, "capsule top {max}");
    }
}
