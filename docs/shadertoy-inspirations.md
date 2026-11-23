# Shadertoy inspiration collection

> Rendering effect analysis and principle notes

---

## 1. [MdVXzw] Deep Blue Floating Rectangles

**URL:** https://www.shadertoy.com/view/MdVXzw

### Effect description

Many semi-transparent blue small rectangles float on deep blue background, overall atmosphere quiet, abstract. Background has slowly flowing cloud / noise texture and soft glow center; rectangles rotate and drift independently, like suspended particles in deep sea or reflections on water.

### Rendering principles

| Element | Implementation |
|------|----------|
| **Background color** | `bgColor = vec3(0.01, 0.16, 0.42)` deep navy |
| **Background texture** | 4 layers FBM (Fractal Brownian Motion) overlay, each transforms UV with rotation matrix `mat2(1.6, 1.2, -1.2, 1.6)`, producing organic cloud patterns |
| **Noise type** | Classic Value Noise: `random()` uses `fract(sin(dot))*43758.5453` hash, bilinear interpolation smoothing |
| **Ripple distortion** | FBM result added as UV offset, producing water-like distortion |
| **Glow** | At `glowPos = vec2(-2., 0.)` compute falloff by dot product, multiplied by FBM value |
| **Rectangle generation** | Loop 60 rectangles: each uses `random(vec2(index))` to decide size, X position and rotation speed |
| **Rectangle motion** | X direction `velX = -iTime/8.` drift left; Y direction `sin(index*rnd*1000 + velY)` sine sway |
| **Rectangle rendering** | `rectangle()` uses `smoothstep` for soft edges (with blur param), color multiplied by `pos.z/maxSize` for size→brightness mapping |
| **Post-processing** | `sqrt(abs(col))` approximate gamma correction, softer colors |

### Points to borrow for Prisma

- **Soft rectangle SDF technique** (`smoothstep` edge blur) for UI / particles
- **FBM + rotation matrix** noise composition, suitable for clouds, water, smoke
- **Batch small-object loop + pseudo-random attribute assignment** pattern, suitable for particle system prototypes

---

## 2. [XsK3RR] 1D Real-Time Lighting

**URL:** https://www.shadertoy.com/view/XsK3RR

### Effect description

2D SDF scene + multi-light real-time rendering. Image composed of circles, rings, rectangles via CSG, lit by 6 colored dynamic lights with real-time shadows. Overall like 2D light-and-shadow sandbox.

### Rendering principles

#### scene build  (SDF + CSG)

| Element | Implementation |
|------|------|
| **Base SDF** | `sdCircle` / `sdRing` / `sdBox` / `sdRect` / `sdPlane` |
| **Boolean ops** | `opU` = `min` (union), `opI` = `max` (intersection), `opS` = `max(-a, b)` (subtraction) |
| **Domain repetition** | `Rep1` (1D periodic), `Rep2` (2D grid) |
| **Scene composition** | Screen borders as walls + small dot grid + thin line grid + hollow center box + ring + rotating animated rectangle cut |

#### 1D shadow map (core innovation)

- For **each light**, precompute a **1D texture**: angle θ → nearest occluder distance at that angle
- `SampleShadow()` function:
  1. Convert fragment-to-light vector to polar `(angle, radius)`
  2. Sample 1D shadow map by angle `texture(iChannel0, vec2(angle, light_id))` to get nearest distance `s`
  3. `1 - smoothstep(s, s+0.02, radius)` outputs shadow value (0 = in shadow, 1 = lit)
- Complexity: O(n) per light per pixel, very efficient for 2D scenes

#### Lighting calculation

```glsl
l = 0.01 / pow(length(vec3(uv - o, 0.1)), 2.0);  // inverse-square falloff
l *= SampleShadow(i, uv - o);                      // shadow occlusion
b += LightColor(i) * l;                            // accumulate
```

- `AMBIENT_LIGHT = vec3(0.1)` ambient base
- 6 lights, position / color stored in `iChannel0` texture buffer
- `z=0.1` in falloff prevents singularity (division by zero)

#### Shading

- `mix(FLOOR_COLOR, WALL_COLOR, smoothstep(psz, 0.0, d))` — mix floor (inside) and wall (edge) by SDF sign
- Final color = material color × accumulated lighting

### Points to borrow for Prisma

- **1D shadow mapping**: for 2D games / UI scenes lighter than traditional 2D shadow maps, no sampling aliasing
- **SDF + CSG scene description**: concise and elegant, for 2D prototyping tools in editor
- **Texture buffer as GPU data store**: `iChannel0` stores light array and shadow map, lightweight SSBO alternative
- **Complete 2D lighting pipeline of inverse-square + ambient + shadow**, directly mappable to 2D render pass in `prism-render`

---

## 3. [XsK3RR] CRT Demo Reel

**URL:** https://www.shadertoy.com/view/Xs3GWj
**Author:** David A Roberts

### Effect description

Loops 5 purely procedural black-white / color patterns with retro CRT display effect (screen curvature, vignette, scanlines, pixel grid). 10-second cycle, pixelation transition at start / end, simulating old monitor boot / signal switching.

### Overall architecture

```
Each frame → 4×SSAA → CRT curved UV → pattern selector → CRT post-process → output
         │                              │
     (AA=4, manual loop)         switch one demo every 5 seconds
                              (mod(0.1*iTime, 5))
```

### 5 procedural patterns

| # | Name | Principle | Palette |
|---|------|------|------|
| 1 | **margarita** | Spiral distance field `z = length(p) - 3.5·atan(p) + sin x + cos y`, modulo `z` generates spiral stripes | Red / black / white |
| 2 | **plaid_meltdown** | Two diagonal waveforms `cube_root(sin(2.5√2·(x±y)))` + sine warp `2·sin(x·sin y + y·sin x)` compared with high-freq noise | Black / white |
| 3 | **sunlight_revealed** | Multiple angle modulo ops generate colored rays (blue / red / yellow), plus log-spiral regions | Blue + red + yellow + white |
| 4 | **threesome** | Product of `sin(length)·cos(8·angle)` at three centers forms interference pattern, threshold cut | Black / white |
| 5 | **digital_bacteria** | `floor`-based cell grid + trig condition checks, organic colony shapes | Yellow / brown / red / green |

### CRT simulation layers

| Effect | Implementation |
|------|------|
| **Screen curvature** | `CRTCurveUV` — UV quadratic offset outward from center, simulates CRT sphere |
| **Vignette** | `DrawVignette` — 0.3 power of `uv.x·uv.y·(1-uv.x)·(1-uv.y)` |
| **Scanlines** | `cos(π·y·240)` horizontal scanlines, slight temporal jitter |
| **Pixel grid** | `cos(π·x·640)` vertical simulates CRT shadow mask / grid |
| **Pixelation transition** | First / last 2 seconds of every 10s: `floor(p·scale)/scale` lowers virtual resolution, simulates signal switching |

### Points to borrow for Prisma

- **Pure procedural pattern generation**: no textures, for loading screens, UI backgrounds, cover art
- **CRT post-process chain**: can be wrapped as group of fullscreen post-process effects (curvature, vignette, scanlines, grid)
- **Multi-demo loop scheduling**: `mod` + time segments managing different render modes, for engine cutscene / demo system
- **Manual SSAA**: `#define AA 4` + loop accumulate average, reference when no built-in MSAA

---

## 4. [43cBzn] Ripple Box Field

**URL:** https://www.shadertoy.com/view/43cBzn
**Reference:** tssSDN, Qiita (domain repetition + grid stepping optimization)

### Effect description

In 3D scene a small red ball flies above grid of boxes, boxes undulate by distance to ball — like water ripples spreading. Each box independently bounces up / down, organic wave propagation. Simple diffuse lighting, different facing surfaces colored differently.

### Rendering principles

#### scene structure

```
scene = moving ball (red) ∪ infinitely repeated box array (grid boxes)
```

#### Domain repetition

```glsl
vec2 id = floor(q.xz / rep);            // cell coords
q.xz = mod(q.xz, rep) - rep * 0.5;      // map to [-rep/2, rep/2]
```

- Render infinite grid with **1 SDF box**
- `rep = 0.04` — cell spacing

#### Ripple animation

```glsl
float bsDist = length(spo.xz - bcp.xz);                // ball-to-cell-center distance
float s = smoothstep(0., 0.5, bsDist);                 // smooth falloff
float height = sin(hash * PI2 + t * (2. + bsDist * 0.015)) * 0.05;
q.y -= 0.125 - height * (1. - pow(s, 0.9));            // box height offset
```

- Each box height phase from `hash` (pseudo-random by cell coords)
- Frequency varies slightly with `bsDist`, producing **ripple spread**
- `smoothstep` makes distant boxes settle first, near ones move first

#### Grid-aware raymarching optimization

```glsl
accDist += min(
    min(
        (step(0., rd.x) - mod(p.x, rep)) * rdi.x,   // distance to next x grid line
        (step(0., rd.z) - mod(p.z, rep)) * rdi.z    // distance to next z grid line
    ) + 0.0001,
    dist                                            // SDF distance
);
```

- Standard SDF raymarch may skip thin boxes because SDF can be inaccurate with repeated objects
- Extra stepping to nearest grid boundary ensures thin geometry isn't missed

#### Lighting

| Element | Implementation |
|------|------|
| **Diffuse** | `dot(l, n) * 0.5 + 0.5` (half-Lambert style) |
| **Ball material** | mat=0 → red |
| **Box material** | mat=1 → colored by normal: `n.x>0.5`=red, `n.y>0.5`=white, `n.z>0.5`=dark red |
| **Gamma correction** | `pow(col, vec3(0.4545))` |

#### camera

- `ro = vec3(1, 1, 1.2)`, looking at `ta = vec3(0, 0.2, 0)`
- Uses `getRayDir` + `camera` matrix to build ray direction
- Supports mouse drag rotation (currently commented out: `m.x * 0.`)

### Points to borrow for Prisma

- **Domain repetition + pseudo-random attributes**: infinite grid needs only one SDF, for grass, cities, particle fields
- **Distance-driven animation**: drive animation params by object-to-event-source distance, naturally produces ripple / wave propagation
- **Grid-aware stepping**: prevent missing thin objects in repeated-domain raymarch, directly usable in `prism-render` SDF rendering
- **`minMat` pattern**: return `vec2(dist, materialID)` for unified material management, suits ECS material system design

---

## 5. [MdX3Rr] Procedural Terrain + Motion Blur

**URL:** https://www.shadertoy.com/view/MdX3Rr
**Author:** Inigo Quilez (2016)
**Reference:** iquilezles.org — derivatives noise, soft shadow, fog, lighting, terrain raymarching

### Effect description

Two-pass pipeline: **Pass 1** renders procedurally generated mountain terrain (with clouds, snow, lighting, soft shadows, fog), encoding screen-space velocity into alpha channel; **Pass 2** reads Pass 1 output, applies linear motion blur to moving pixels, adds vignette and tonemap. Simulates cinematic aerial flyover of mountains.

---

### Pass 1 — terrain render main pass

`vec4 render()` outputs `vec4(color, encoded_velocity)`

#### Terrain generation

Build FBM using **analytic derivatives of Value Noise** (`noised()` returns `vec3(value, dx, dy)`):

```glsl
for (int i = 0; i < 16; i++) {
    vec3 n = noised(p);
    d += n.yz;                          // accumulate derivatives for domain warp
    a += b * n.x / (1.0 + dot(d, d));  // division creates steep features
    b *= 0.5;
    p = m2 * p * 2.0;                   // rotate and scale
}
```

| Element | Details |
|------|------|
| **Noise source** | `iChannel0` 256×256 random texture, `texelFetch`, `&255` wrapping |
| **FBM layers** | High-precision `terrainH` = 16, medium `terrainM` = 9, coarse `terrainL` = 3 |
| **Domain warp** | Per-layer accumulate derivative `d` for `1/(1+d·d)` division, sharp ridges |
| **Scale** | `SC = 250.0`, terrain height ≈ `SC × 120 × FBM` |

Three precision terrain functions used for: normal calculation (high), raycast / shadow (medium), camera height (low).

#### Raymarching

```glsl
t += 0.4 * h;     // h = pos.y - terrainM(pos.xz)
// convergence condition: abs(h) < 0.0015 * t
```

- Standard "up / down" relaxed raymarching
- 300-step cap, distance-adaptive convergence tolerance

#### Lighting system

| Element | Implementation |
|------|------|
| **Main light** | `light1 = normalize(vec3(-0.8, 0.4, -0.3))` directional |
| **Diffuse** | `clamp(dot(light1, nor), 0, 1)` |
| **Ambient** | `0.5 + 0.5 × nor.y`, blue tint `vec3(0.4, 0.6, 1.0)` |
| **Backlight** | dot normal with horizontal reverse of light |
| **Soft shadow** | IQ classic `softShadow`: 80 steps, `min(16·h/t)` accumulation |
| **Specular** | Blinn-Phong (half-vector), power 16 + Fresnel |

#### Material layering

```
rock base → grass / soil slope → snow cap
```

- **Rock**: random texture fluctuation + normal y controlled blend
- **Grass slope**: `smoothstep(0.7, 0.9, nor.y)` blends green tint
- **Snow cap**: height + FBM perturbation + normal slope control, product of `h`, `e`, `o` factors

#### Sky

```glsl
col = vec3(0.3, 0.5, 0.85) - rd.y * rd.y * 0.5;      // gradient sky
col += 0.25 * vec3(1.0, 0.7, 0.4) * pow(sundot, 5);   // sun glow
col += 0.25 * vec3(1.0, 0.8, 0.6) * pow(sundot, 64);  // corona
```

- Clouds: FBM sampling + horizon blend
- Horizon fog: `pow(1 - max(rd.y, 0), 16)` blends toward blue

#### Fog (distance fog)

```glsl
float fo = 1.0 - exp(-pow(0.001 * t / SC, 1.5));
col = mix(col, fco, fo);
```

Exponential-power fog, sub-linear growth with distance.

#### Velocity vector encoding

```glsl
// old frame camera
float oldTime = time - 0.1 * 1.0/24.0;
vec3 wpos = ro + rd * t;                                    // world-space position
vec3 cpos = oldCam * (wpos - oldRo);                         // old camera space
vec2 npos = oldFl * cpos.xy / cpos.z;                        // NDC
vec2 spos = 0.5 + 0.5 * npos * vec2(iResolution.y/iResolution.x, 1.0);  // screen space

// encode as single float
vec2 uv = fragCoord / iResolution.xy;
spos = clamp(0.5 + 0.5 * (spos - uv) / 0.25, 0.0, 1.0);
vel = floor(spos.x * 1023.0) + floor(spos.y * 1023.0) * 1024.0;
```

- Screen velocity from reprojecting current-frame position under **old-frame camera matrix**
- 2×10-bit packed into 1 float
- Sky pixels `vel = -1.0` (no motion blur)

---

### Pass 2 — Motion Blur post-processing

(same as above, abbreviated as summary)

#### Data layout

```
data.xyz = color (RGB)
data.w   = encoded_velocity (or <0 = static)
```

#### Process

1. **Velocity decode**: `mod(w, 1024)/1023` + `floor(w/1024)/1023` → `[-0.25, 0.25]`
2. **Motion blur**: 32 taps linearly sampled and averaged along velocity
3. **Vignette**: `0.5 + 0.5 × pow(16·x·y·(1-x)·(1-y), 0.1)`
4. **Tonemap**: `col*0.6 + 0.4*col²(3-2col) + vec3(0,0,0.04)` (S-curve + cool tint)

---

### Points to borrow for Prisma

- **Two-pass pipeline structure**: Pass1 render + encode → Pass2 consume, fits `prism-render` graph architecture
- **Value Noise analytic derivatives + domain-warp terrain**: `1/(1+d·d)` creates sharp ridges, for procedural terrain system
- **Soft shadows**: IQ classic 80-step soft shadow, mappable as engine shadow pass option
- **Single float encoding 2×10-bit velocity vector**: bandwidth trick, for motion vector pass
- **Layered material blending**: height / slope / FBM-driven rock→grass→snow, reusable as terrain layer blending system
- **Exponential fog**: `1 - exp(-pow(d, 1.5))` more natural than linear fog
- **Custom tonemap**: linear + smoothstep + color mask, as built-in tonemap option

---

## 6. [4ttSWf] Procedural Rainforest

**URL:** https://www.shadertoy.com/view/4ttSWf
**Author:** Inigo Quilez
**Reference:** iquilezles.org — derivatives noise, soft shadow, fog, lighting, terrain raymarching

### Effect description

Fully procedural rainforest: undulating mountain terrain covered by thousands of individual trees, volumetric clouds in distance, soft natural lighting. Camera slowly pans, with temporal AA (TAA-like temporal reprojection) and fine post grading. No external textures, all math-generated.

### Render architecture

```
Each frame → AA jitter → raymarch terrain / trees → volumetric clouds → post-process (fog + grading) → temporal reprojection blend → output
                                                                           │
                                                          iChannel0 stores previous-frame camera matrix + color
```

---

### Key infrastructure

#### 3D Value Noise + analytic derivatives

```glsl
vec4 noised(in vec3 x) {
    // returns vec4(value, dx, dy, dz)
    // uses cubic / quintic interpolation + hash lookup
}
```

- 3D version: `n = p.x + 317*p.y + 157*p.z` as hash seed, 8 corners lookup
- 2D version: 4 corners lookup
- Both return analytic derivatives for FBM accumulation and normal calc

#### FBM with Derivative Accumulation

| Function | Dimension | Layers | Purpose |
|------|------|------|------|
| `fbm_4` | 2D/3D | 4 | Tree distribution, detail perturbation |
| `fbmd_7` | 3D | 7 | Terrain bump map |
| `fbmd_8` | 3D | 8 | Volumetric cloud density |
| `fbmd_9` | 2D | 9 | Terrain height |
| `fbm_9` | 2D | 9 | Terrain height (no derivatives) |

- Uses fixed rotation matrices `m3`/`m2` and inverses, rotating each layer's UV
- Supports skipping loop on first frame via `ZERO = min(iFrame,0)` (Shadertoy compat)

---

### Terrain system

```
terrainMap(p) → vec2(height, slope_flag)
terrainMapD(p) → vec4(height, normal)   // analytic normal
```

- 9-layer FBM base height, range ≈ 600m
- `smoothstep(0.12, 0.13, abs(e+0.12))` marks **high-slope areas** (cliffs)
- Cliff boost: `+90 * smoothstep(552, 594, height)`
- Adaptive step: `t += dis * 0.8 * (1 - 0.75 * slope_flag)` — smaller steps on steep slopes for precision

#### Raymarching

```glsl
for (400 steps) {
    dis = pos.y - terrainMap(pos.xz).x;
    if (dis < 0.001*t) break;
    t += dis * 0.8 * (1 - 0.75*slope);
}
```

- Linear interpolation recomputes intersection (sub-step precision)
- Simultaneously tracks tree envelope for next-step tree-marching start point

---

### Tree system

#### Distribution

- 2D grid domain repetition: `floor(p.xz / 2.0)` + `fract(p.xz / 2.0)`
- 4 neighbors (2×2) lookup, `hash2` decides each tree's:
  - Position offset `o = hash2(n+g)`
  - Height `kMaxTreeHeight * (0.4 + 0.8*v.x)` (≈ 1.92 - 5.76)
  - Width `0.5 + 0.2*v.x + 0.3*v.y` (≈ 0.5 - 1.0)
- Large-scale FBM controls growth density: `bb < 0` canopy halved, `bb > 0` trunk doubled

#### geometry

```glsl
float k = sdEllipsoidY(q, vec2(width, 0.5*height));
```

- Each tree as **single ellipsoid** SDF
- Far (`rt < 1200`) apply FBM perturbation for natural canopy

#### material

- Trunk / canopy color from `mid` (material ID: `hash` decides) blend
- Large-scale FBM (`brownAreas`) controls brown bark regions
- Near colors more saturated, far toward uniform tone

---

### Volumetric clouds

| Element | Details |
|------|------|
| **Height range** | 600m - 1200m |
| **Density field** | `fbmd_8` 8-layer FBM, domain warp (time animation) |
| **Stepping** | up to 128 steps, adaptive |
| **Lighting** | diffuse (`dot(nor, sunDir)`) + ambient + half-transmission |
| **Self-shadow** | sample density field `cloudsMap` along light direction |
| **Composition** | Front-to-back alpha blending |
| **Fresnel** | `clamp(1+dot(nor,rd))` rim light |

---

### Lighting model

#### Terrain lighting

```glsl
// normal = terrain normal + bump map (fbmd_7 derivative noise)
nor = normalize(tnor + 0.8*(1-abs(tnor.y))*0.8*fbmd_7(...).yzw);

float dif = clamp(dot(nor, sunDir), 0, 1) * sha1 * sha2;  // diffuse × shadow
float bac = clamp(dot(-sunDir_horiz, nor), 0, 1);          // backlight
float dom = clamp(0.5 + 0.5*nor.y, 0, 1);                  // skylight
float foc = clamp((pos.y/2 - 180)/130, 0, 1);              // height focus

vec3 lin = 0.2 * mix(0.1*green, 3.0*blue, dom) * foc    // ambient
         + 8.5 * vec3(1,0.9,0.8) * dif                  // sunlight
         + 0.27 * vec3(1.1,1.0,0.9) * bac * foc;        // backlight
```

- `LOWQUALITY` macro controls shadow quality (32 vs 128 steps)
- Terrain and trees have separate shadow-stepping functions

#### Tree lighting

- Transmission shadow: `sha2` blends `a + (1-a)*sha2`, simulating light through canopy

---

### Post-processing and grading

```glsl
// gamma
col = pow(clamp(col*1.1-0.02, 0, 1), vec3(0.4545));

// contrast (smoothstep curve)
col = col*col*(3.0-2.0*col);

// grading
col = pow(col, vec3(1.0, 0.92, 1.0));  // slight green tint
col *= vec3(1.02, 0.99, 0.9);           // red / yellow tone
col.z += 0.1;                            // blue shift
```

---

### Temporal reprojection

```glsl
// store camera matrix to texture, first 3 rows
if (ip.y==0 && ip.x<=2)
    fragColor = vec4(ca[ip.x], -dot(ca[ip.x], ro));

// next frame: read old camera matrix, reproject
mat3x4 oldCam = mat3x4(texelFetch(iChannel0, ivec2(0,0), 0), ...);
vec4 wpos = vec4(ro + rd*resT, 1.0);
vec3 cpos = (wpos * oldCam);                          // old camera space
vec2 npos = 1.5 * cpos.xy / cpos.z;                   // NDC
vec2 spos = 0.5 + 0.5*npos * aspect;                  // screen space

// blend
col = mix(ocol, col, 0.1 + 0.8*isCloud);
```

- First row of pixels each frame stores 3×4 camera matrix to `iChannel0`
- Equivalent to **custom TAA**: blend with previous frame (higher weight in cloud regions)
- Effectively reduces noise, especially on shadows and tree edges

---

### Points to borrow for Prisma

- **Procedural forest**: grid domain repetition + ellipsoid SDF + random attributes, directly for vegetation system
- **Value Noise analytic derivative system**: `noised` + `fbmd_*` throughout, unified noise infrastructure
- **Temporal reprojection TAA**: implemented with few pixel rows storing camera matrix, very low cost, suitable engine built-in
- **Volumetric cloud pipeline**: adaptive stepping + self-shadow + front-to-back composition, extractable as general volumetric render pass
- **Bump-map blending**: `terrainNor + fbmd_7.yzw * (1-abs(tnor.y))` layered detail normals
- **LOWQUALITY tiering**: macro switch shadow / stepping precision, for desktop vs Android adaptation
- **Grading trio**: smoothstep contrast + channel multiply + blue shift, concise effective stylized post-process

---

## 7. [MdBGzG] Canyon

**URL:** https://www.shadertoy.com/view/MdBGzG
**Author:** Inigo Quilez (2014)
**Reference:** iquilezles.org — texture filtering, triplanar mapping, Oren-Nayar diffuse

### Effect description

Procedurally generated canyon: steep rock walls rise from valley floor, rich texture detail, sunlight from one side casts long shadows. Camera advances slowly along canyon path, view gradually rises from valley floor, revealing full canyon. Warm dry palette, Mexico / American Southwest canyon style.

### Render architecture

```
Each frame → AA loop → camera path → raymarch (SDF) → material lookup (triplanar) → Oren-Nayar lighting → soft shadow + sky → fog → post-process
                                                                                                       │
                                                                                        textures from iChannel0/1/2/3
```

---

### Noise and bit shifting

Use **texture lookup to implement Value Noise** (`iChannel2` holds 256×256 random image):

```glsl
float noise1(in vec3 x) {
    vec3 p = floor(x);
    vec3 f = fract(x);
    f = f*f*(3.0-2.0*f);
    vec2 uv = (p.xy + vec2(37.0,17.0)*p.z) + f.xy;
    vec2 rg = textureLod(iChannel2, (uv+0.5)/256.0, 0.0).yx;
    return mix(rg.x, rg.y, f.z);
}
```

- Use `vec2(37,17)*p.z` to unfold z dimension into xy plane, implementing 3D noise lookup
- `noise1()` faster than pure math hash (hardware texture filtering)

**Displacement FBM** (3–4 layers, rotation matrix `m`):

```glsl
f = 0.5000*noise1(p); p = m*p*2.02;
f += 0.2500*noise1(p); p = m*p*2.03;
f += 0.1250*noise1(p); p = m*p*2.01;
```

---

### Terrain system

```glsl
vec4 map(in vec3 p) {
    float h = terrain(p.xz);               // base terrain height (texture lookup)
    float dis = displacement(0.25*p*vec3(1,4,1));  // displacement noise
    dis *= 3.0;
    return vec4((dis + p.y - h) * 0.25, p.x, h, 0.0);
}
```

| Element | Implementation |
|------|------|
| **Base height** | `terrain()` uses `iChannel0` (large-scale heightmap) + `iChannel1` (detail) two smoothsteps |
| **Cliff / slope** | naturally formed by noise gradient, no artificial cliff boost |
| **Displacement detail** | 3–4 layer FBM displacement, anisotropic scale `vec3(1,4,1)` (vertical stretch) |
| **LOWDETAIL** | fewer displacement layers, fewer stepping iterations |

#### Raymarching

- 256 steps (LOWDETAIL) or 512 (HIGH_QUALITY)
- Adaptive step: `t += tmp.x * 0.7` (high quality) or `t += tmp.x` (normal)
- Convergence: `tmp.x < 0.001 * t`

---

### Triplanar mapping

```glsl
vec4 texcube(sampler2D sam, vec3 p, vec3 n) {
    vec4 x = texture(sam, p.yz);
    vec4 y = texture(sam, p.zx);
    vec4 z = texture(sam, p.xy);
    return (x*abs(n.x) + y*abs(n.y) + z*abs(n.z)) / (abs(n.x)+abs(n.y)+abs(n.z));
}
```

- Sample from three directions, blend by normal weights — eliminate UV seams
- `texcubeGrad` version passes `dpdx/dpdy`, supports anisotropic filtering

**Manual texture filtering** `textureGood()`:

```glsl
uv = uv*1024.0 - 0.5;
vec2 iuv = floor(uv);
vec2 f = fract(uv);
// manual 4-point bilinear fetch
```

- Avoid texture bleeding from distant terrain mipmap transitions

---

### camera

```glsl
vec3 cpath(float t) {
    vec3 pos = vec3(0, 0, 95 + t);
    float a = smoothstep(5, 30, t);
    pos.xz += a*150 * cos(vec2(5,6) + 0.01*t);
    pos.xz -= a*150 * cos(vec2(5,6));
    pos.xz += a*50 * cos(vec2(0,3.5) + 6*0.01*t);
    pos.xz -= a*50 * cos(vec2(0,3.5));
    return pos;
}
```

- **Path blending**: use `smoothstep` to fade between two path segments
- First straight, then gradually add curved offset
- Camera height `ro.y = terrain2(ro.xz) - 0.5` flies near ground

---

### material system

Multi-layer blend (all triplanar texture lookups):

```
rock base texture → vegetation cover (green/brown) → bright sand → micro detail
```

```glsl
// 1. base rock texture
vec3 te = texcubeGrad(iChannel0, 0.15*pos, nor, ...).xyz;
mate.xyz = 0.6 * te;

// 2. vegetation (large-scale noise control)
float th = texcubeGrad(iChannel0, 0.002*pos, nor, ...).x;
vec3 dcol = mix(vec3(0.2,0.3,0.0), 0.4*vec3(0.65,0.4,0.2), 0.2+0.8*th);
mate.xyz = mix(mate.xyz, 2.0*dcol, th * smoothstep(0,1,nor.y));

// 3. sand brightening
float rr = texcubeGrad(iChannel1, 0.04*pos, nor, ...).y;
mate.xyz *= mix(vec3(1.0), 1.5*vec3(0.25,0.24,0.22)*1.5, rr);
```

**Bump map**:

```glsl
// use textureGrad with iChannel0 for derivative normal perturbation
bnor.x = texcubeGrad(..., pos+vec3(be,0,0), ...) - texcubeGrad(..., pos-vec3(be,0,0), ...);
// ...
nor = normalize(nor + amo*(bnor - nor*dot(bnor,nor)));
```

- Gram-Schmidt correction: `nor + amo*(bnor - nor*dot(bnor,nor))` keeps normal normalized after perturbation

---

### Lighting

| Element | Implementation |
|------|------|
| **Diffuse** | **Oren-Nayar** model (rough surface) → `diffuse()` function, roughness `r=1.0` |
| **Skylight** | 5-direction hemisphere sampling + Oren-Nayar |
| **Backlight** | `blig = normalize(vec3(-klig.x, 0, -klig.z))` horizontal reverse |
| **Soft shadow** | `softshadow(pos+0.01*nor, klig, 0.005, k=64)`, 50 steps, `k` controls hardness |
| **Specular** | `mate.w * pow(clamp(dot(reflect, klig),0,1), 2) * clamp(dot(nor,klig),0,1)` |

#### Oren-Nayar diffuse

```glsl
float diffuse(in vec3 l, in vec3 n, in vec3 v, float r) {
    float r2 = r*r;
    float a = 1.0 - 0.5*(r2/(r2+0.57));
    float b = 0.45*(r2/(r2+0.09));
    float nl = dot(n,l);
    float nv = dot(n,v);
    float ga = dot(v-n*nv, n-n*nl);
    return max(0,nl) * (a + b*max(0,ga) * sqrt((1-nv*nv)*(1-nl*nl)) / max(nl, nv));
}
```

- More realistic rough-surface (rock, sand) than Lambert
- `r=1.0` = very rough

#### light source  → shadow color map

```glsl
lin += 7.0*dif*vec3(1.20,0.50,0.25) * vec3(sha, sha*0.5+0.5*sha*sha, sha*sha);
```

- Shadow color has **warm tone falloff**: shadows reddish / warm, simulating light scattering through dust

#### Sky

```glsl
vec3 hor = mix(1.2*vec3(0.70,1.0,1.0), vec3(1.5,0.5,0.2), 0.25+0.75*sun);
vec3 col = mix(vec3(0.2,0.6,.9), hor, exp(-(4+2*(1-sun))*max(0,rd.y-0.1)));
```

- Horizon color blends with sun angle (cool blue → warm yellow)
- Sun glow: power 4 / 32 / 512 triple rings

**Clouds**: simple texture lookup `iChannel0` for single-layer cloud

---

### Post-process chain

```glsl
col *= 1.0 - 0.25*pow(1.0-clamp(dot(cam[2],klig),0,1), 3.0);  // lens flare mask

col = pow(max(col,0), vec3(0.45));   // gamma
col *= vec3(1.1, 1.0, 1.0);          // warm tone
col = clamp(col, 0, 1);
col = col*col*(3.0-2.0*col);          // contrast (smoothstep)
col = pow(col, vec3(0.9, 1.0, 1.0)); // brighten red / green channels
col = mix(col, vec3(dot(col, vec3(0.333))), 0.4);  // 40% desaturation
col = col*0.5 + 0.5*col*col*(3.0-2.0*col);         // contrast again
tot *= 0.3 + 0.7*pow(16*q.x*q.y*(1-q.x)*(1-q.y), 0.1);  // vignette
```

- Strong stylized grading: warm tint + desaturation + double contrast curves
- Vignette multiplied after AA accumulation

---

### Points to borrow for Prisma

- **Triplanar mapping**: essential technique eliminating terrain UV seams, can be wrapped as engine utility
- **Texture as noise**: precomputed random texture replaces math hash, better performance (especially mobile)
- **Oren-Nayar diffuse**: better than Lambert for rock / sand / rough materials, as BRDF option
- **Manual texture filtering**: `textureGood()` avoids mipmap transition issues, useful for distant terrain textures
- **Camera path blending**: `smoothstep` + `cos` smooth path transitions, for engine cutscene system
- **Material layering**: base texture → vegetation → sand → micro detail multi-layer blend pattern
- **Gram-Schmidt normal perturbation**: `nor + amo*(bnor - nor*dot(bnor,nor))` preserves orthogonality
- **Shadow color mapping**: color shifts with shadow depth (warm falloff), improves lighting realism

---

## 8. [MdlGW7] River Plain

**URL:** https://www.shadertoy.com/view/MdlGW7
**Author:** Inigo Quilez (2013)
**Reference:** iquilezles.org — volumetric rendering, noise-based trees, god rays

### Effect description

Open plain with a river flowing through, dense forests on both banks, volumetric clouds and scattered sunlight in sky. Camera flies circular orbit, sun from one side forms **god rays** through cloud gaps. Trees rendered as voxel density fields, not individual geometry. Two-pass pipeline with velocity encoding (same as MdX3Rr).

### Overall architecture

```
Raymarch → lake / river → terrain → volumetric trees → volumetric clouds + god rays → velocity encode → Pass2 motion blur
                                                                         (same MdX3Rr post-pass)
```

---

### Terrain and river

```glsl
float envelope(vec3 p) {
    float isLake = 1.0 - smoothstep(0.62, 0.72, textureLod(iChannel0, 0.001*p.zx, 0.0).x);
    return 0.1 + isLake * 0.9 * textureLod(iChannel1, 0.01*p.xz, 0.0).x;
}
```

| Element | Details |
|------|------|
| **Terrain height** | `envelope()` = base 0.1 + non-lake × 0.9 × height texture |
| **River / lake** | `iChannel0` large-scale texture `smoothstep(0.62, 0.72)` binarized marks water |
| **Water rendering** | planar reflection (perturbed normal) + sun specular + Fresnel + cloud shadow overlay |

#### Water-surface normal

```glsl
nor.xz = 0.10*(-1+2*texture(iChannel3, 1.5*pos.xz).xz);
nor.xz += 0.15*(-1+2*texture(iChannel3, 3.2*pos.xz).xz);
nor.xz += 0.20*(-1+2*texture(iChannel3, 6.0*pos.xz).xz);
nor = normalize(nor);
```

Three texture layers overlay perturbation, simulating water ripples.

---

### Trees: voxel density field (not geometry)

Most unique technique here — **trees aren't independent SDFs, but voxel density fields**:

```glsl
vec4 mapTrees(vec3 pos, vec3 rd) {
    float r = clamp(h / e, 0, 1);                     // normalized tree height
    float den = smoothstep(r, 1.0, texture(iChannel0, pos.xz*0.15).x);  // density
    den *= 1.0 - 0.95 * clamp((r-0.75)/(1-0.75), 0, 1);  // top taper

    // lighting
    vec3 nor = calcNormal(pos);
    vec3 dif = vec3(1.0) * clamp(dot(nor, lig), 0, 1);
    float amb = 0.5 + 0.5*nor.y;

    // cloud shadow cast onto trees
    float w = (2.8-pos.y)/lig.y;
    float c = fbm((pos+w*lig)*0.35);
    c = smoothstep(0.38, 0.6, c);
    dif *= pow(vec3(c), vec3(0.8, 1.0, 1.5));

    // BRDF
    vec3 brdf = 1.7*vec3(1.5,1.0,0.8)*dif*(0.1+0.9*oc) + 1.3*amb*vec3(0.1,0.15,0.2)*oc;
    vec3 mate = 0.6*vec3(0.5,0.5,0.1) + 0.3*texture(iChannel1, 0.1*pos.xz).zyx;
    col = brdf * mate;

    den *= 1.0 - isLake;  // no trees in water
    return vec4(col, den);
}
```

#### Volumetric trees vs SDF trees comparison

| Feature | MdlGW7 volumetric trees | 4ttSWf SDF ellipsoid trees |
|------|-------------|-----------------|
| **Per-tree independence** | ❌ continuous density field | ✅ independent ellipsoids |
| **Geometry precision** | low (blurry canopy) | medium (distinguishable canopy shape) |
| **Stepping cost** | 512 steps front-to-back | SDF raymarch + shadows |
| **Suitable distance** | mid-far | mid-near |

#### Volumetric tree stepping

```glsl
for (512 steps) {
    vec4 col = mapTrees(pos, rd);
    col.xyz = mix(col.xyz, bgcol, 1-exp(-0.0018*t*t));  // fog blend
    col.rgb *= col.a;
    sum = sum + col*(1-sum.a);                           // front-to-back
    t += 0.0035*t;                                       // exponential step
}
```

---

### Volumetric clouds

- 4-layer FBM density field, height range `[2.5, 3.1]`
- Cloud self-shadow: sample FBM along light direction
- 64 steps front-to-back alpha blending
- Sun tint: `col.xyz += vec3(1,0.7,0.4)*0.4*pow(sun, 6)*(1-col.w)`

### God rays

```glsl
// accumulate during cloud stepping:
rays += 0.02 * smoothstep(0.38, 0.6, c) * (1-col.a) * (1-smoothstep(2.75, 2.8, pos.y));

// final overlay:
col += (1-0.8*col) * rays*rays*rays * 0.4 * vec3(1.0, 0.8, 0.7);
```

- Accumulate in cloud gaps (`1-col.a`) facing light (`c` passes threshold)
- `rays³` sharpens ray shape
- `(1-0.8*col)` strengthens bright regions, suppresses dark

---

### Lighting overview

| Element | Implementation |
|------|------|
| **Light** | `lig = normalize(vec3(0.7, 0.4, 0.2))` warm directional |
| **Sky** | `vec3(0.84,0.95,1.0)*0.77 - rd.y*0.6` gradient + sun glow |
| **Tree lighting** | terrain-normal diffuse + ambient + cloud shadow transmission |
| **Water** | reflect sun + Fresnel + cloud shadow |
| **Fog** | `1-exp(-0.0018*t*t)` exponential-square fog |

### Velocity encoding

Same as MdX3Rr, but precision reduced to **2×8-bit** (255):

```glsl
vel = floor(spos.x*255.0) + floor(spos.y*255.0)*256.0;
```

### post-processing

```glsl
col = pow(col, vec3(0.45));                    // gamma
col = col*0.1 + 0.9*col*col*(3.0-2.0*col);     // contrast (smoothstep 90%)
col = mix(col, vec3(luminance), 0.2);           // 20% desaturation
col *= vec3(1.06, 1.05, 1.0);                   // warm tone
```

---

### Points to borrow for Prisma

- **Volumetric tree rendering**: alpha-blended density field replacing independent SDF trees, for mid-far vegetation — scalable quality / performance
- **God rays**: accumulate rays during volumetric stepping + post overlay, more realistic than standalone post-process
- **Voxel density field vs SDF choice**: can switch by distance in same scene (near SDF detail, far volumetric), LOD thinking
- **Water ripple normals**: multi-layer texture overlay perturbation, simple effective
- **Velocity encoding precision adjustable**: 8-bit vs 10-bit, quality / bandwidth tradeoff
- **Exponential step**: `t += 0.0035*t` auto-balances near / far sampling density in volumetric stepping

---

# Appendix: Terrain erosion and heightfield techniques summary

> Based on analysis of above 8 shaders, extract terrain height generation and erosion simulation techniques as reference for Prisma heightmap generator.

---

## I. Heightfield generation approach comparison

| Approach | Representative shader | Principle | Pros / cons |
|------|-----------|------|--------|
| **FBM pure math noise** | MdX3Rr | Multi-layer Value / Perlin noise overlay, rotation matrix frequency change | Fully procedural, infinite terrain; domain warp for ridges |
| **Texture lookup noise** | MdBGzG, MdlGW7 | `texelFetch` / `textureLod` reading precomputed random texture | Faster (hardware texture filtering), precision limited by texture size |
| **Hybrid: texture base + FBM detail** | MdBGzG | `terrain()`=texture lookup + `displacement()`=FBM | Both large-scale controllability and small-scale randomness |
| **SDF domain repetition + pseudo-random height** | 43cBzn | `mod(p, rep)` infinite grid, `rand(id)` height | For artificial regular structures (cities, platforms), not natural terrain |

**Recommended engine approach**: hybrid — texture base provides editable large-scale shape, FBM / domain warp provides natural detail.

---

## II. FBM parameterization comparison

| Shader | Function | Layers | Rotation matrix | Freq multiplier | Amp decay | Special handling |
|--------|------|------|----------|----------|----------|----------|
| MdX3Rr | `fbm` / `noised` | 4-16 | `mat2(0.8, -0.6, 0.6, 0.8)` | 2.0 | 0.5 | Domain warp `1/(1+d·d)` sharp ridges |
| 4ttSWf | `fbmd_9` | 9 | `m2` / `m2i` | 1.9 | 0.55 | Analytic derivative accumulation → normals |
| MdBGzG | `displacement` | 3-4 | IQ `m` matrix | 2.0 | 0.5 | Anisotropic scale `vec3(1,4,1)` |
| MdlGW7 | `fbm` | 4 | IQ `m` matrix | 2.0 | 0.5 | Pure value noise no derivatives |
| 4ttSWf(cld) | `fbmd_8` | 8 | `m3` / `m3i` | 2.0 | 0.65 | 7 layers derivative accumulation, 8th value only |

**Key parameter empirical values**:
- Frequency multiplier ≈ 1.9~2.0
- Amp decay ≈ 0.5~0.65 (smaller = less detail but smoother)
- Layers: 3-4 (rough) / 7-9 (high quality) / 16 (extreme detail)

---

## III. Erosion simulation techniques (procedural, not physical simulation)

These shaders don't contain real water / thermal erosion particle simulation, but **fake erosion appearance** with following tricks:

### 1. Ridges / gullies: domain warping

**MdX3Rr core code**:
```glsl
vec2 d = vec2(0);
for (int i = 0; i < 16; i++) {
    vec3 n = noised(p);
    d += n.yz;                      // accumulate gradient
    a += b * n.x / (1.0 + dot(d, d)); // division creates sharp valleys / ridges
    b *= 0.5;
    p = m2 * p * 2.0;
}
```

**Principle**: when `d` (accumulated gradient) large, `1/(1+d·d)` drops sharply — lowers height at steep slopes forming **gullies**; at gradient reversal (ridge line) less lowering forming **ridges**. Simulates most intuitive result of water erosion: slopes cut into gullies, ridges preserved.

### 2. Cliff bands

**MdX3Rr**:
```glsl
e += 90.0 * smoothstep(552.0, 594.0, e);  // sudden height boost in specific band
```

**4ttSWf**:
```glsl
float a = 1.0 - smoothstep(0.12, 0.13, abs(e + 0.12));  // mark steep regions
// during stepping:
t += dis * 0.8 * (1.0 - 0.75 * a);  // smaller steps on steep slopes
```

**Principle**: discontinuous offset in specific height band simulates cliffs from stratum uplift / faulting. `smoothstep` smooths transition avoiding aliasing.

### 3. Normal-driven material blend (visual erosion)

All terrain shaders blend materials by **height + slope + noise**, visually simulating erosion:

```glsl
// 4ttSWf: rock→grass→snow
float h = smoothstep(55.0, 80.0, pos.y/SC + 25.0*fbm(0.01*pos.xz/SC));
float e = smoothstep(1.0-0.5*h, 1.0-0.1*h, nor.y);
col = mix(col, 0.29*vec3(0.62,0.65,0.7), smoothstep(0.1, 0.9, h*e*o));
// steep slope (nor.y small) → exposed rock
// gentle slope (nor.y large) → vegetation / snow
```

### 4. Anisotropic perturbation

**MdBGzG**:
```glsl
float dis = displacement(0.25 * p * vec3(1.0, 4.0, 1.0)) * 3.0;
return vec4((dis + p.y - h) * 0.25, ...);
```

Anisotropic scale `vec3(1, 4, 1)` makes vertical perturbation stronger, simulating vertical joints / cracks.

---

## IV. Recommended Heightmap Generator implementation path

### Core pipeline

```
Input params → multi-layer noise composition → domain warp / ridges → cliff boost → erosion post-process → output heightmap
```

### Layered design

| Layer | Function | Recommended technique |
|----|------|----------|
| **Layer 0: base** | Continent / island outline | Low-freq FBM (2–3 layers) or predefined mask texture |
| **Layer 1: mountain system** | Main mountain direction | Domain-warp FBM (4–6 layers), `1/(1+d·d)` ridge generation |
| **Layer 2: detail** | Rock / gully high-freq detail | Anisotropic FBM displacement |
| **Layer 3: cliff** | Steep slope boost | `smoothstep` height-band offset |
| **Post: erosion** | Water / thermal erosion sim | Optional physical sim or pure visual fake |

### Output format

```
heightmap (f32 / channel) → optionally export as:
  ├── .png/.exr grayscale (for engine terrain system)
  ├── .obj mesh (for preview)
  └── embedded into engine terrain asset format
```

### Algorithm capability tiers

| Tier | FBM layers | Domain warp | Stepping precision | Platform |
|------|----------|--------|----------|----------|
| Low | 3-4 | ❌ | rough | Android mobile |
| Medium | 6-8 | ✅ light | medium | mid-range desktop |
| High | 9-16 | ✅ full | fine | high-end desktop / offline baking |

### Math implementation reference

- **Noise function**: Value Noise (fastest) → Perlin → Simplex (quality progression)
- **Domain warp**: borrow MdX3Rr derivative accumulation + `1/(1+d·d)`
- **FBM loop**: rotation matrices (IQ `m2`/`m3`) avoid axis bias
- **Cliff**: `height += cliff_amount * smoothstep(cliff_bottom, cliff_top, height)`
- **Ridge noise**: `1 - abs(noise)` or domain-warp approach

---





