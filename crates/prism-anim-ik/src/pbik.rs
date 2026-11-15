//! Position-Based Inverse Kinematics (PBIK) solver for multi-effector
//! full-body chains (GASP P5).

/// Single joint in a PBIK chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbikJoint {
    pub position: [f32; 3],
    pub parent: Option<usize>,
    pub inv_mass: f32,
}

/// Effector pin target for a joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbikEffector {
    pub joint_index: usize,
    pub target: [f32; 3],
    pub weight: f32,
}

/// Solve a PBIK skeleton with `iterations` relaxation passes.
pub fn solve_pbik(joints: &mut [PbikJoint], effectors: &[PbikEffector], iterations: u32) {
    if joints.is_empty() || effectors.is_empty() {
        return;
    }
    // Capture rest bone lengths.
    let rest_lengths: Vec<f32> = joints
        .iter()
        .map(|j| match j.parent {
            Some(p) => dist(j.position, joints[p].position),
            None => 0.0,
        })
        .collect();

    for _ in 0..iterations.max(1) {
        // 1. Pull effectors toward targets.
        for eff in effectors {
            if let Some(j) = joints.get_mut(eff.joint_index) {
                let w = eff.weight.clamp(0.0, 1.0);
                for k in 0..3 {
                    j.position[k] += (eff.target[k] - j.position[k]) * w;
                }
            }
        }

        // 2. Enforce bone-length distance constraints.
        for i in 0..joints.len() {
            let Some(p) = joints[i].parent else {
                continue;
            };
            let rest = rest_lengths[i];
            if rest <= 1e-6 {
                continue;
            }
            let w_i = joints[i].inv_mass;
            let w_p = joints[p].inv_mass;
            let w_sum = w_i + w_p;
            if w_sum <= 1e-6 {
                continue;
            }
            let delta = [
                joints[i].position[0] - joints[p].position[0],
                joints[i].position[1] - joints[p].position[1],
                joints[i].position[2] - joints[p].position[2],
            ];
            let len = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
            if len <= 1e-6 {
                continue;
            }
            let corr = (len - rest) / (len * w_sum);
            for (k, component) in delta.iter().enumerate() {
                let d = component * corr;
                joints[i].position[k] -= d * w_i;
                joints[p].position[k] += d * w_p;
            }
        }
    }
}

#[inline]
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbik_reaches_effector_and_preserves_bone_lengths() {
        let mut joints = vec![
            PbikJoint {
                position: [0.0, 0.0, 0.0],
                parent: None,
                inv_mass: 0.0,
            },
            PbikJoint {
                position: [0.0, 1.0, 0.0],
                parent: Some(0),
                inv_mass: 1.0,
            },
            PbikJoint {
                position: [0.0, 2.0, 0.0],
                parent: Some(1),
                inv_mass: 1.0,
            },
        ];
        let effectors = [PbikEffector {
            joint_index: 2,
            target: [1.0, 1.0, 0.0],
            weight: 1.0,
        }];
        solve_pbik(&mut joints, &effectors, 16);
        assert!(dist(joints[2].position, [1.0, 1.0, 0.0]) < 0.05);
        assert!((dist(joints[1].position, joints[0].position) - 1.0).abs() < 0.05);
        assert!((dist(joints[2].position, joints[1].position) - 1.0).abs() < 0.05);
    }
}
