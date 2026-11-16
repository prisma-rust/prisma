//! Pure-CPU IBL baking and `CookedIbl` binary format (`PIBL`).
//!
//! Shared between the offline build pipeline (`prism-build-pipeline`), which
//! bakes `.hdr` environment maps into `.pak` packages, and the Vulkan renderer
//! (`prism-render`), which uploads the pre-convolved `f16` textures directly
//! to GPU staging buffers.

use anyhow::{bail, ensure, Context as _};

/// Default environment cubemap face resolution.
pub const ENV_FACE_SIZE: u32 = 512;
/// Diffuse irradiance cubemap face resolution.
pub const IRRADIANCE_FACE_SIZE: u32 = 64;
/// Stratified hemisphere samples per axis^2 for irradiance convolution.
pub const IRRADIANCE_SAMPLES: u32 = 64;
/// Specular prefiltered cubemap base face resolution.
pub const PREFILTERED_FACE_SIZE: u32 = 128;
/// Specular prefiltered cubemap mip count.
pub const PREFILTERED_MIP_LEVELS: u32 = 5;
/// Importance samples per texel for prefiltered environment convolution.
pub const PREFILTERED_SAMPLES: u32 = 128;
/// Split-sum BRDF LUT width/height.
pub const BRDF_LUT_SIZE: u32 = 512;
/// Importance samples per texel for BRDF LUT integration.
pub const BRDF_LUT_SAMPLES: u32 = 1024;

/// Magic bytes identifying a cooked `PIBL` payload.
pub const COOKED_IBL_MAGIC: [u8; 4] = *b"PIBL";
/// Binary format version for `CookedIbl`.
pub const COOKED_IBL_VERSION: u32 = 1;

const HEADER_BYTES: usize = 4 + 6 * 4;

/// Pre-convolved IBL textures in `f16` (`u16` bits), ready for direct GPU upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookedIbl {
    pub env_face_size: u32,
    pub irradiance_face_size: u32,
    pub prefiltered_face_size: u32,
    pub prefiltered_mip_levels: u32,
    pub brdf_size: u32,
    /// `6 * env_face_size^2 * 4` `f16` values (`R16G16B16A16_SFLOAT`).
    pub env_cube: Vec<u16>,
    /// `6 * irradiance_face_size^2 * 4` `f16` values (`R16G16B16A16_SFLOAT`).
    pub irradiance_cube: Vec<u16>,
    /// One buffer per mip level (`6 * (prefiltered_face_size >> mip).max(1)^2 * 4` `f16` values).
    pub prefiltered_mips: Vec<Vec<u16>>,
    /// `brdf_size^2 * 2` `f16` values (`R16G16_SFLOAT`).
    pub brdf_lut: Vec<u16>,
}

impl CookedIbl {
    /// Returns `true` when `bytes` begins with the `PIBL` magic header.
    pub fn is_cooked(bytes: &[u8]) -> bool {
        bytes.starts_with(&COOKED_IBL_MAGIC)
    }

    /// Loads a `CookedIbl` from either pre-baked `PIBL` bytes or raw `.hdr` bytes.
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        if Self::is_cooked(bytes) {
            Self::decode(bytes)
        } else {
            Self::bake_from_hdr(bytes)
        }
    }

    /// Loads only the `env_cube` (for Path Tracing / skybox-only pipelines),
    /// leaving `irradiance_cube`, `prefiltered_mips`, and `brdf_lut` as 1x1
    /// neutral dummies without decoding or convolving PBR IBL data.
    pub fn from_bytes_env_only(bytes: &[u8]) -> anyhow::Result<Self> {
        if Self::is_cooked(bytes) {
            Self::decode_env_only(bytes)
        } else {
            Self::bake_env_only_from_hdr(bytes)
        }
    }

    /// Decodes an RGBE `.hdr` image and bakes only `env_cube` on the CPU.
    pub fn bake_env_only_from_hdr(hdr_bytes: &[u8]) -> anyhow::Result<Self> {
        let (rgba_f32, width, height) =
            crate::hdr::load_rgbe(hdr_bytes).context("failed to decode environment .hdr")?;
        Ok(Self::bake_env_only_from_equirect(&rgba_f32, width, height))
    }

    /// Bakes only `env_cube` from a procedural gradient sky equirectangular map.
    pub fn bake_procedural_env_only() -> Self {
        let (rgba_f32, width, height) = procedural_equirect(512, 256);
        Self::bake_env_only_from_equirect(&rgba_f32, width, height)
    }

    fn bake_env_only_from_equirect(eq: &[f32], width: u32, height: u32) -> Self {
        let env_f32 = generate_cubemap(eq, width, height, ENV_FACE_SIZE);
        Self::env_only_with_cube(ENV_FACE_SIZE, f32_slice_to_f16(&env_f32))
    }

    fn env_only_with_cube(env_face_size: u32, env_cube: Vec<u16>) -> Self {
        Self {
            env_face_size,
            irradiance_face_size: 1,
            prefiltered_face_size: 1,
            prefiltered_mip_levels: 1,
            brdf_size: 1,
            env_cube,
            irradiance_cube: vec![0; 6 * 4],
            prefiltered_mips: vec![vec![0; 6 * 4]],
            brdf_lut: vec![f32_to_f16(1.0); 2],
        }
    }

    /// Decodes an RGBE `.hdr` image and bakes all IBL textures on the CPU.
    pub fn bake_from_hdr(hdr_bytes: &[u8]) -> anyhow::Result<Self> {
        let (rgba_f32, width, height) =
            crate::hdr::load_rgbe(hdr_bytes).context("failed to decode environment .hdr")?;
        Ok(Self::bake_from_equirect(&rgba_f32, width, height))
    }

    /// Bakes all IBL textures from a procedural gradient sky equirectangular map.
    pub fn bake_procedural() -> Self {
        let (rgba_f32, width, height) = procedural_equirect(512, 256);
        Self::bake_from_equirect(&rgba_f32, width, height)
    }

    /// Bakes all IBL textures from a linear RGBA `f32` equirectangular buffer.
    pub fn bake_from_equirect(eq: &[f32], width: u32, height: u32) -> Self {
        let env_f32 = generate_cubemap(eq, width, height, ENV_FACE_SIZE);
        let irr_f32 = convolve_irradiance(eq, width, height, IRRADIANCE_FACE_SIZE);
        let pre_f32 = convolve_prefiltered(
            eq,
            width,
            height,
            PREFILTERED_FACE_SIZE,
            PREFILTERED_MIP_LEVELS,
        );
        let brdf_f32 = compute_brdf_lut(BRDF_LUT_SIZE);

        Self {
            env_face_size: ENV_FACE_SIZE,
            irradiance_face_size: IRRADIANCE_FACE_SIZE,
            prefiltered_face_size: PREFILTERED_FACE_SIZE,
            prefiltered_mip_levels: PREFILTERED_MIP_LEVELS,
            brdf_size: BRDF_LUT_SIZE,
            env_cube: f32_slice_to_f16(&env_f32),
            irradiance_cube: f32_slice_to_f16(&irr_f32),
            prefiltered_mips: pre_f32.iter().map(|m| f32_slice_to_f16(m)).collect(),
            brdf_lut: f32_slice_to_f16(&brdf_f32),
        }
    }

    /// Serializes this `CookedIbl` into the `PIBL` binary format.
    pub fn encode(&self) -> Vec<u8> {
        let payload_u16s = self.env_cube.len()
            + self.irradiance_cube.len()
            + self.prefiltered_mips.iter().map(Vec::len).sum::<usize>()
            + self.brdf_lut.len();
        let mut out = Vec::with_capacity(HEADER_BYTES + payload_u16s * 2);

        out.extend_from_slice(&COOKED_IBL_MAGIC);
        out.extend_from_slice(&COOKED_IBL_VERSION.to_le_bytes());
        out.extend_from_slice(&self.env_face_size.to_le_bytes());
        out.extend_from_slice(&self.irradiance_face_size.to_le_bytes());
        out.extend_from_slice(&self.prefiltered_face_size.to_le_bytes());
        out.extend_from_slice(&self.prefiltered_mip_levels.to_le_bytes());
        out.extend_from_slice(&self.brdf_size.to_le_bytes());

        write_u16_slice(&mut out, &self.env_cube);
        write_u16_slice(&mut out, &self.irradiance_cube);
        for mip in &self.prefiltered_mips {
            write_u16_slice(&mut out, mip);
        }
        write_u16_slice(&mut out, &self.brdf_lut);
        out
    }

    /// Deserializes and validates a `PIBL` binary payload.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        Self::decode_inner(bytes, true)
    }

    /// Validates a `PIBL` header and reads only `env_cube`, skipping the PBR
    /// convolution sections.
    pub fn decode_env_only(bytes: &[u8]) -> anyhow::Result<Self> {
        Self::decode_inner(bytes, false)
    }

    fn decode_inner(bytes: &[u8], include_pbr: bool) -> anyhow::Result<Self> {
        ensure!(
            bytes.len() >= HEADER_BYTES,
            "cooked IBL truncated header ({} bytes)",
            bytes.len()
        );
        ensure!(
            bytes[..4] == COOKED_IBL_MAGIC,
            "invalid cooked IBL magic header"
        );

        let mut cursor = 4usize;
        let version = read_u32(bytes, &mut cursor);
        if version != COOKED_IBL_VERSION {
            bail!("unsupported cooked IBL version {version} (expected {COOKED_IBL_VERSION})");
        }
        let env_face_size = read_u32(bytes, &mut cursor);
        let irradiance_face_size = read_u32(bytes, &mut cursor);
        let prefiltered_face_size = read_u32(bytes, &mut cursor);
        let prefiltered_mip_levels = read_u32(bytes, &mut cursor);
        let brdf_size = read_u32(bytes, &mut cursor);

        ensure!(
            env_face_size > 0
                && irradiance_face_size > 0
                && prefiltered_face_size > 0
                && prefiltered_mip_levels > 0
                && brdf_size > 0,
            "cooked IBL dimensions must be non-zero"
        );

        let env_len = cube_rgba_u16_len(env_face_size)?;
        let irr_len = cube_rgba_u16_len(irradiance_face_size)?;
        let mut pre_lens = Vec::with_capacity(prefiltered_mip_levels as usize);
        let mut total_pre_u16 = 0usize;
        for mip in 0..prefiltered_mip_levels {
            let fs = (prefiltered_face_size >> mip).max(1);
            let len = cube_rgba_u16_len(fs)?;
            total_pre_u16 = total_pre_u16
                .checked_add(len)
                .context("cooked IBL prefiltered size overflow")?;
            pre_lens.push(len);
        }
        let brdf_len = (brdf_size as usize)
            .checked_mul(brdf_size as usize)
            .and_then(|n| n.checked_mul(2))
            .context("cooked IBL brdf size overflow")?;

        let total_u16 = env_len
            .checked_add(irr_len)
            .and_then(|n| n.checked_add(total_pre_u16))
            .and_then(|n| n.checked_add(brdf_len))
            .context("cooked IBL total size overflow")?;
        let expected_bytes = HEADER_BYTES
            .checked_add(
                total_u16
                    .checked_mul(2)
                    .context("cooked IBL byte overflow")?,
            )
            .context("cooked IBL byte overflow")?;

        ensure!(
            bytes.len() == expected_bytes,
            "cooked IBL length mismatch: got {} bytes, expected {}",
            bytes.len(),
            expected_bytes
        );

        let env_cube = read_u16_vec(bytes, &mut cursor, env_len);
        if !include_pbr {
            return Ok(Self::env_only_with_cube(env_face_size, env_cube));
        }
        let irradiance_cube = read_u16_vec(bytes, &mut cursor, irr_len);
        let mut prefiltered_mips = Vec::with_capacity(pre_lens.len());
        for len in pre_lens {
            prefiltered_mips.push(read_u16_vec(bytes, &mut cursor, len));
        }
        let brdf_lut = read_u16_vec(bytes, &mut cursor, brdf_len);

        Ok(Self {
            env_face_size,
            irradiance_face_size,
            prefiltered_face_size,
            prefiltered_mip_levels,
            brdf_size,
            env_cube,
            irradiance_cube,
            prefiltered_mips,
            brdf_lut,
        })
    }
}

fn cube_rgba_u16_len(face_size: u32) -> anyhow::Result<usize> {
    (face_size as usize)
        .checked_mul(face_size as usize)
        .and_then(|n| n.checked_mul(6 * 4))
        .context("cooked IBL cubemap size overflow")
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> u32 {
    let v = u32::from_le_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap());
    *cursor += 4;
    v
}

fn write_u16_slice(out: &mut Vec<u8>, data: &[u16]) {
    for &v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn read_u16_vec(bytes: &[u8], cursor: &mut usize, len: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(len);
    for chunk in bytes[*cursor..*cursor + len * 2].chunks_exact(2) {
        out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    *cursor += len * 2;
    out
}

/// Converts a slice of `f32` values into IEEE-754 `f16` bit patterns.
pub fn f32_slice_to_f16(data: &[f32]) -> Vec<u16> {
    data.iter().copied().map(f32_to_f16).collect()
}

/// IEEE-754 single -> half (round to nearest even).
pub fn f32_to_f16(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;

    if exp == 255 {
        return sign | 0x7c00;
    }
    let e = exp - 127 + 15;
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = 14 - e;
        let half_mant = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let rounded =
            if rem > (1 << (shift - 1)) || (rem == (1 << (shift - 1)) && (half_mant & 1) != 0) {
                half_mant + 1
            } else {
                half_mant
            };
        return sign | (rounded as u16);
    }
    if e >= 31 {
        return sign | 0x7c00;
    }
    let half_mant = mant >> 13;
    let rem = mant & 0x1fff;
    let rounded = if rem > 0x1000 || (rem == 0x1000 && (half_mant & 1) != 0) {
        half_mant + 1
    } else {
        half_mant
    };
    sign | ((e as u16) << 10) | (rounded as u16)
}

// ---------------------------------------------------------------------------
// CPU convolution helpers
// ---------------------------------------------------------------------------

/// Cosine-weighted hemisphere convolution of the equirect into a diffuse
/// irradiance cubemap.
pub fn convolve_irradiance(eq: &[f32], eq_w: u32, eq_h: u32, face_size: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (6 * face_size * face_size * 4) as usize];
    let sqrt_samples = (IRRADIANCE_SAMPLES as f32).sqrt() as u32;
    let theta_steps = sqrt_samples;
    let phi_steps = sqrt_samples;
    let inv_theta = 1.0 / theta_steps as f32;
    let inv_phi = 1.0 / phi_steps as f32;

    for f in 0..6u32 {
        for y in 0..face_size {
            for x in 0..face_size {
                let u = 2.0 * (x as f32 + 0.5) / face_size as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / face_size as f32 - 1.0;
                let n = normalize3(cube_direction(f, u, v));
                let (t, b) = build_tangent_frame(n);

                let mut sum = [0.0f32; 3];
                let mut total_weight = 0.0f32;

                for ti in 0..theta_steps {
                    let theta = (ti as f32 + 0.5) * inv_theta * (std::f32::consts::PI * 0.5);
                    let sin_theta = theta.sin();
                    let cos_theta = theta.cos();
                    for pj in 0..phi_steps {
                        let phi = (pj as f32 + 0.5) * inv_phi * 2.0 * std::f32::consts::PI;
                        let l_local = [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta];
                        let l = normalize3([
                            t[0] * l_local[0] + b[0] * l_local[1] + n[0] * l_local[2],
                            t[1] * l_local[0] + b[1] * l_local[1] + n[1] * l_local[2],
                            t[2] * l_local[0] + b[2] * l_local[1] + n[2] * l_local[2],
                        ]);
                        let weight = dot3(n, l).max(0.0);
                        if weight > 0.0 {
                            let ueq = l[2].atan2(l[0]) / (2.0 * std::f32::consts::PI) + 0.5;
                            let veq = l[1].acos().clamp(0.0, 1.0) / std::f32::consts::PI;
                            let c = sample_equirect_bilinear(eq, eq_w, eq_h, ueq, veq);
                            sum[0] += c[0] * weight;
                            sum[1] += c[1] * weight;
                            sum[2] += c[2] * weight;
                            total_weight += weight;
                        }
                    }
                }

                let inv_w = if total_weight > 0.0 {
                    1.0 / total_weight
                } else {
                    0.0
                };
                let o = ((f * face_size * face_size + y * face_size + x) * 4) as usize;
                out[o] = sum[0] * inv_w;
                out[o + 1] = sum[1] * inv_w;
                out[o + 2] = sum[2] * inv_w;
                out[o + 3] = 1.0;
            }
        }
    }
    out
}

/// GGX importance-sampled prefiltered environment map.
/// Returns one RGBA f32 buffer per mip level.
pub fn convolve_prefiltered(
    eq: &[f32],
    eq_w: u32,
    eq_h: u32,
    face_size: u32,
    mip_levels: u32,
) -> Vec<Vec<f32>> {
    let mut out = Vec::with_capacity(mip_levels as usize);
    for mip in 0..mip_levels {
        let fs = (face_size >> mip).max(1);
        let roughness = if mip_levels > 1 {
            let t = mip as f32 / (mip_levels - 1) as f32;
            t * t
        } else {
            0.0
        };
        let mut mip_data = vec![0.0f32; (6 * fs * fs * 4) as usize];

        for f in 0..6u32 {
            for y in 0..fs {
                for x in 0..fs {
                    let u = 2.0 * (x as f32 + 0.5) / fs as f32 - 1.0;
                    let v = 2.0 * (y as f32 + 0.5) / fs as f32 - 1.0;
                    let n = normalize3(cube_direction(f, u, v));
                    let (t, b) = build_tangent_frame(n);

                    let mut sum = [0.0f32; 3];
                    let mut count = 0u32;

                    for i in 0..PREFILTERED_SAMPLES {
                        let xi = hammersley(i, PREFILTERED_SAMPLES);
                        let h_local = importance_sample_ggx(xi, roughness);
                        let v_dot_h = h_local[2];
                        let l_local = [
                            2.0 * v_dot_h * h_local[0],
                            2.0 * v_dot_h * h_local[1],
                            2.0 * v_dot_h * h_local[2] - 1.0,
                        ];
                        if l_local[2] <= 0.0 {
                            continue;
                        }
                        let l = normalize3([
                            t[0] * l_local[0] + b[0] * l_local[1] + n[0] * l_local[2],
                            t[1] * l_local[0] + b[1] * l_local[1] + n[1] * l_local[2],
                            t[2] * l_local[0] + b[2] * l_local[1] + n[2] * l_local[2],
                        ]);
                        let ueq = l[2].atan2(l[0]) / (2.0 * std::f32::consts::PI) + 0.5;
                        let veq = l[1].acos().clamp(0.0, 1.0) / std::f32::consts::PI;
                        let c = sample_equirect_bilinear(eq, eq_w, eq_h, ueq, veq);
                        sum[0] += c[0];
                        sum[1] += c[1];
                        sum[2] += c[2];
                        count += 1;
                    }

                    let inv_n = if count > 0 { 1.0 / count as f32 } else { 0.0 };
                    let o = ((f * fs * fs + y * fs + x) * 4) as usize;
                    mip_data[o] = sum[0] * inv_n;
                    mip_data[o + 1] = sum[1] * inv_n;
                    mip_data[o + 2] = sum[2] * inv_n;
                    mip_data[o + 3] = 1.0;
                }
            }
        }
        out.push(mip_data);
    }
    out
}

/// Compute the BRDF integration LUT (split-sum approximation).
/// Returns RG f32 data for the 2D texture (`size x size`).
pub fn compute_brdf_lut(size: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (size * size * 2) as usize];
    let inv_size = 1.0 / size as f32;

    for y in 0..size {
        let roughness = (y as f32 + 0.5) * inv_size;
        for x in 0..size {
            let n_dot_v = (x as f32 + 0.5) * inv_size;
            let v = [(1.0 - n_dot_v * n_dot_v).sqrt().max(0.0), 0.0, n_dot_v];

            let mut scale = 0.0f32;
            let mut bias = 0.0f32;

            for i in 0..BRDF_LUT_SAMPLES {
                let xi = hammersley(i, BRDF_LUT_SAMPLES);
                let h = importance_sample_ggx(xi, roughness);
                let v_dot_h = v[0] * h[0] + v[1] * h[1] + v[2] * h[2];
                let l = [
                    2.0 * v_dot_h * h[0] - v[0],
                    2.0 * v_dot_h * h[1] - v[1],
                    2.0 * v_dot_h * h[2] - v[2],
                ];
                let n_dot_l = l[2].max(0.0);
                let n_dot_h = h[2].max(0.0);
                let v_dot_h = v_dot_h.max(0.0);

                if n_dot_l > 0.0 {
                    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
                    let g_v = n_dot_v / (n_dot_v * (1.0 - k) + k);
                    let g_l = n_dot_l / (n_dot_l * (1.0 - k) + k);
                    let g = g_v * g_l;
                    let g_vis = g * v_dot_h / (n_dot_h * n_dot_v.max(1e-6));
                    let fc = (1.0 - v_dot_h).powi(5);
                    scale += (1.0 - fc) * g_vis;
                    bias += fc * g_vis;
                }
            }

            let inv_n = 1.0 / BRDF_LUT_SAMPLES as f32;
            let o = (y * size + x) as usize * 2;
            out[o] = scale * inv_n;
            out[o + 1] = bias * inv_n;
        }
    }
    out
}

/// Build a cubemap (6 faces x `face_size`^2 RGBA) from an equirectangular HDR buffer.
pub fn generate_cubemap(eq: &[f32], eq_w: u32, eq_h: u32, face_size: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (6 * face_size * face_size * 4) as usize];
    for f in 0..6u32 {
        for y in 0..face_size {
            for x in 0..face_size {
                let u = 2.0 * (x as f32 + 0.5) / face_size as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / face_size as f32 - 1.0;
                let d = normalize3(cube_direction(f, u, v));
                let ueq = d[2].atan2(d[0]) / (2.0 * std::f32::consts::PI) + 0.5;
                let veq = (d[1].acos() / std::f32::consts::PI).clamp(0.0, 1.0);
                let c = sample_equirect_bilinear(eq, eq_w, eq_h, ueq, veq);
                let o = ((f * face_size * face_size + y * face_size + x) * 4) as usize;
                out[o..o + 4].copy_from_slice(&c);
            }
        }
    }
    out
}

/// Procedural equirectangular environment (gradient sky) used when no `.hdr` asset is supplied.
pub fn procedural_equirect(width: u32, height: u32) -> (Vec<f32>, u32, u32) {
    let mut data = vec![0.0f32; (width * height * 4) as usize];
    for y in 0..height {
        let v = (y as f32 + 0.5) / height as f32;
        let theta = v * std::f32::consts::PI;
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let phi = u * 2.0 * std::f32::consts::PI;
            let dir = [
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ];
            let up = dir[1];
            let sky = [
                0.25 + 0.55 * (0.5 + 0.5 * up),
                0.35 + 0.55 * (0.5 + 0.5 * up),
                0.55 + 0.45 * (0.5 + 0.5 * up),
            ];
            let i = ((y * width + x) * 4) as usize;
            data[i] = sky[0];
            data[i + 1] = sky[1];
            data[i + 2] = sky[2];
            data[i + 3] = 1.0;
        }
    }
    (data, width, height)
}

fn build_tangent_frame(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let up = if n[1].abs() < 0.9999 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let t = normalize3(cross3(up, n));
    let b = cross3(n, t);
    (t, b)
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn hammersley(i: u32, n: u32) -> [f32; 2] {
    [i as f32 / n as f32, radical_inverse_vdc(i)]
}

fn radical_inverse_vdc(mut bits: u32) -> f32 {
    bits = bits.rotate_right(16);
    bits = ((bits & 0x55555555) << 1) | ((bits & 0xAAAAAAAA) >> 1);
    bits = ((bits & 0x33333333) << 2) | ((bits & 0xCCCCCCCC) >> 2);
    bits = ((bits & 0x0F0F0F0F) << 4) | ((bits & 0xF0F0F0F0) >> 4);
    bits = ((bits & 0x00FF00FF) << 8) | ((bits & 0xFF00FF00) >> 8);
    bits as f32 * 2.328_306_4e-10
}

fn importance_sample_ggx(xi: [f32; 2], roughness: f32) -> [f32; 3] {
    let a = roughness * roughness;
    let a2 = a * a;
    let phi = 2.0 * std::f32::consts::PI * xi[0];
    let cos_theta = ((1.0 - xi[1]) / (1.0 + (a2 - 1.0) * xi[1])).sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
    [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta]
}

fn cube_direction(face: u32, u: f32, v: f32) -> [f32; 3] {
    match face {
        0 => [1.0, -v, -u],
        1 => [-1.0, -v, u],
        2 => [u, 1.0, v],
        3 => [u, -1.0, -v],
        4 => [u, -v, 1.0],
        _ => [-u, -v, -1.0],
    }
}

fn normalize3(d: [f32; 3]) -> [f32; 3] {
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-8);
    [d[0] / len, d[1] / len, d[2] / len]
}

fn sample_equirect_bilinear(eq: &[f32], w: u32, h: u32, u: f32, v: f32) -> [f32; 4] {
    let fw = w as f32;
    let fh = h as f32;
    let x = u * fw - 0.5;
    let y = v * fh - 0.5;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let x0w = (((x0 % w as i32) + w as i32) % w as i32) as u32;
    let x1w = (x0w + 1) % w;
    let y0c = y0.clamp(0, h as i32 - 1) as u32;
    let y1c = (y0 + 1).clamp(0, h as i32 - 1) as u32;
    let idx = |xx: u32, yy: u32| -> usize { (yy * w + xx) as usize * 4 };
    let c00 = idx(x0w, y0c);
    let c10 = idx(x1w, y0c);
    let c01 = idx(x0w, y1c);
    let c11 = idx(x1w, y1c);
    let mut out = [0.0f32; 4];
    for i in 0..4 {
        let a = eq[c00 + i] * (1.0 - fx) + eq[c10 + i] * fx;
        let b = eq[c01 + i] * (1.0 - fx) + eq[c11 + i] * fx;
        out[i] = a * (1.0 - fy) + b * fy;
    }
    out
}

#[cfg(test)]
#[path = "ibl_tests.rs"]
mod tests;
