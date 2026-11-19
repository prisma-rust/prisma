// Neural Radiance Cache (NRC) compute shaders — RTXGI v2.0 style online
// multi-resolution hash-grid + 2-hidden-layer MLP with Adam weight updates.
//
// Entry points:
//   1. nrcClearMain - deterministic Xavier-like weight init + zero Adam moments
//   2. nrcTrainMain - forward pass + backprop + Adam update on training samples
//   3. nrcInferMain - inference pass for secondary-bounce radiance queries

static const uint NRC_HASH_LEVELS = 4u;
static const uint NRC_HASH_SIZE   = 16384u; // per level
static const uint NRC_FEAT_DIM    = 2u;     // per level -> 8 hash features
static const uint NRC_IN_DIM      = 12u;    // 8 hash + 4 direction encoding
static const uint NRC_HIDDEN      = 32u;
static const uint NRC_OUT_DIM     = 3u;

// Weight layout offsets in `weights` buffer (total = 12*32 + 32 + 32*32 + 32 + 32*3 + 3 = 1571 floats):
static const uint OFF_W0 = 0u;
static const uint OFF_B0 = OFF_W0 + NRC_IN_DIM * NRC_HIDDEN;   // 384
static const uint OFF_W1 = OFF_B0 + NRC_HIDDEN;               // 416
static const uint OFF_B1 = OFF_W1 + NRC_HIDDEN * NRC_HIDDEN;  // 1440
static const uint OFF_W2 = OFF_B1 + NRC_HIDDEN;               // 1472
static const uint OFF_B2 = OFF_W2 + NRC_HIDDEN * NRC_OUT_DIM; // 1568
static const uint TOTAL_WEIGHTS = OFF_B2 + NRC_OUT_DIM;       // 1571

public struct NrcUbo
{
    public float4 scene_min_scale; // xyz = scene min, w = inv_scene_extent
    public float4 train_params;    // x = learning_rate, y = beta1, z = beta2, w = eps
    public uint4  counts;          // x = num_train_samples, y = num_infer_queries, z = step_index, w = flags
};

public struct NrcSample
{
    public float4 pos_pad; // xyz = world position
    public float4 dir_pad; // xyz = incoming direction
    public float4 rad_pad; // rgb = target radiance
};

[[vk::binding(0, 0)]]
ConstantBuffer<NrcUbo> nrcUbo;

[[vk::binding(1, 0)]]
RWStructuredBuffer<float> weights;

[[vk::binding(2, 0)]]
RWStructuredBuffer<float> adamM;

[[vk::binding(3, 0)]]
RWStructuredBuffer<float> adamV;

[[vk::binding(4, 0)]]
RWStructuredBuffer<float> hashGrid; // NRC_HASH_LEVELS * NRC_HASH_SIZE * NRC_FEAT_DIM

[[vk::binding(5, 0)]]
StructuredBuffer<NrcSample> trainSamples;

[[vk::binding(6, 0)]]
StructuredBuffer<NrcSample> inferQueries;

[[vk::binding(7, 0)]]
RWStructuredBuffer<float4> inferOutput;

uint spatial_hash(int3 p, uint level)
{
    uint h = uint(p.x) * 1u ^ uint(p.y) * 2654435761u ^ uint(p.z) * 805459861u ^ (level * 19349663u);
    return h % NRC_HASH_SIZE;
}

void encode_input(float3 worldPos, float3 dir, out float x[NRC_IN_DIM])
{
    float3 normPos = (worldPos - nrcUbo.scene_min_scale.xyz) * nrcUbo.scene_min_scale.w;
    for (uint l = 0u; l < NRC_HASH_LEVELS; l++)
    {
        float res = 8.0 * pow(2.0, float(l));
        float3 gp = normPos * res;
        int3 cell = int3(floor(gp));
        uint idx = (l * NRC_HASH_SIZE + spatial_hash(cell, l)) * NRC_FEAT_DIM;
        x[l * 2u + 0u] = hashGrid[idx + 0u];
        x[l * 2u + 1u] = hashGrid[idx + 1u];
    }
    float3 d = normalize(dir);
    x[8]  = d.x;
    x[9]  = d.y;
    x[10] = d.z;
    x[11] = d.x * d.y;
}

void forward_mlp(
    float x[NRC_IN_DIM],
    out float h0[NRC_HIDDEN],
    out float h1[NRC_HIDDEN],
    out float y[NRC_OUT_DIM]
)
{
    for (uint j = 0u; j < NRC_HIDDEN; j++)
    {
        float s = weights[OFF_B0 + j];
        for (uint i = 0u; i < NRC_IN_DIM; i++)
            s += x[i] * weights[OFF_W0 + i * NRC_HIDDEN + j];
        h0[j] = max(s, 0.0);
    }
    for (uint j = 0u; j < NRC_HIDDEN; j++)
    {
        float s = weights[OFF_B1 + j];
        for (uint i = 0u; i < NRC_HIDDEN; i++)
            s += h0[i] * weights[OFF_W1 + i * NRC_HIDDEN + j];
        h1[j] = max(s, 0.0);
    }
    for (uint k = 0u; k < NRC_OUT_DIM; k++)
    {
        float s = weights[OFF_B2 + k];
        for (uint i = 0u; i < NRC_HIDDEN; i++)
            s += h1[i] * weights[OFF_W2 + i * NRC_OUT_DIM + k];
        // Softplus-like positive radiance clamp.
        y[k] = max(s, 0.0);
    }
}

// ---- 1. nrcClearMain ------------------------------------------------------

[shader("compute")]
[numthreads(64, 1, 1)]
void nrcClearMain(uint3 tid : SV_DispatchThreadID)
{
    uint idx = tid.x;
    if (idx < TOTAL_WEIGHTS)
    {
        float sign = ((idx & 1u) == 0u) ? 1.0 : -1.0;
        weights[idx] = sign * 0.05 * frac(sin(float(idx) * 12.9898) * 43758.5453);
        adamM[idx] = 0.0;
        adamV[idx] = 0.0;
    }
    uint totalHash = NRC_HASH_LEVELS * NRC_HASH_SIZE * NRC_FEAT_DIM;
    for (uint h = idx; h < totalHash; h += 64u * 32u)
    {
        hashGrid[h] = 0.01 * (frac(sin(float(h) * 78.233) * 43758.5453) - 0.5);
    }
}

// ---- 2. nrcTrainMain ------------------------------------------------------
// Single-workgroup online SGD/Adam trainer over mini-batch of training samples.

[shader("compute")]
[numthreads(1, 1, 1)]
void nrcTrainMain(uint3 tid : SV_DispatchThreadID)
{
    uint n = nrcUbo.counts.x;
    if (n == 0u)
        return;

    float lr = nrcUbo.train_params.x;
    float beta1 = nrcUbo.train_params.y;
    float beta2 = nrcUbo.train_params.z;
    float eps = nrcUbo.train_params.w;
    float invN = 1.0 / float(n);

    float grad[TOTAL_WEIGHTS];
    for (uint g = 0u; g < TOTAL_WEIGHTS; g++)
        grad[g] = 0.0;

    for (uint s = 0u; s < n; s++)
    {
        NrcSample samp = trainSamples[s];
        float x[NRC_IN_DIM];
        float h0[NRC_HIDDEN];
        float h1[NRC_HIDDEN];
        float y[NRC_OUT_DIM];
        encode_input(samp.pos_pad.xyz, samp.dir_pad.xyz, x);
        forward_mlp(x, h0, h1, y);

        float dy[NRC_OUT_DIM];
        dy[0] = (y[0] - samp.rad_pad.r) * invN;
        dy[1] = (y[1] - samp.rad_pad.g) * invN;
        dy[2] = (y[2] - samp.rad_pad.b) * invN;

        float dh1[NRC_HIDDEN];
        for (uint i = 0u; i < NRC_HIDDEN; i++)
        {
            float acc = 0.0;
            for (uint k = 0u; k < NRC_OUT_DIM; k++)
            {
                grad[OFF_W2 + i * NRC_OUT_DIM + k] += h1[i] * dy[k];
                acc += weights[OFF_W2 + i * NRC_OUT_DIM + k] * dy[k];
            }
            dh1[i] = (h1[i] > 0.0) ? acc : 0.0;
        }
        for (uint k = 0u; k < NRC_OUT_DIM; k++)
            grad[OFF_B2 + k] += dy[k];

        float dh0[NRC_HIDDEN];
        for (uint i = 0u; i < NRC_HIDDEN; i++)
        {
            float acc = 0.0;
            for (uint j = 0u; j < NRC_HIDDEN; j++)
            {
                grad[OFF_W1 + i * NRC_HIDDEN + j] += h0[i] * dh1[j];
                acc += weights[OFF_W1 + i * NRC_HIDDEN + j] * dh1[j];
            }
            dh0[i] = (h0[i] > 0.0) ? acc : 0.0;
            grad[OFF_B1 + i] += dh1[i];
        }

        for (uint i = 0u; i < NRC_IN_DIM; i++)
        {
            for (uint j = 0u; j < NRC_HIDDEN; j++)
                grad[OFF_W0 + i * NRC_HIDDEN + j] += x[i] * dh0[j];
        }
        for (uint j = 0u; j < NRC_HIDDEN; j++)
            grad[OFF_B0 + j] += dh0[j];
    }

    for (uint w = 0u; w < TOTAL_WEIGHTS; w++)
    {
        float g = clamp(grad[w], -10.0, 10.0);
        float m = beta1 * adamM[w] + (1.0 - beta1) * g;
        float v = beta2 * adamV[w] + (1.0 - beta2) * g * g;
        adamM[w] = m;
        adamV[w] = v;
        weights[w] -= lr * m / (sqrt(v) + eps);
    }
}

// ---- 3. nrcInferMain ------------------------------------------------------

[shader("compute")]
[numthreads(64, 1, 1)]
void nrcInferMain(uint3 tid : SV_DispatchThreadID)
{
    uint idx = tid.x;
    if (idx >= nrcUbo.counts.y)
        return;

    NrcSample q = inferQueries[idx];
    float x[NRC_IN_DIM];
    float h0[NRC_HIDDEN];
    float h1[NRC_HIDDEN];
    float y[NRC_OUT_DIM];
    encode_input(q.pos_pad.xyz, q.dir_pad.xyz, x);
    forward_mlp(x, h0, h1, y);
    inferOutput[idx] = float4(y[0], y[1], y[2], 1.0);
}
