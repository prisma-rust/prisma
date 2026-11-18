# 立方体素专用硬件 RTX GI 设计（RT Core 管线 + 体素面缓存 + ReSTIR/DLSS-RR）

> **层级**：L2 专项子系统设计 · 渲染进阶
> **状态**：备选设计蓝图（面向支持硬件光追 RT Core 的桌面 / 主机平台）
> **关联**：[DESIGN.md](DESIGN.md) §6、[rtx-dlss-integration-design.md](rtx-dlss-integration-design.md)、[mobile-raytracing-gi-design.md](mobile-raytracing-gi-design.md)、[voxel-software-gi-design.md](voxel-software-gi-design.md)

本文定义一套充分利用 **NVIDIA RTX RT Core（`VK_KHR_ray_tracing_pipeline` + RayGen/ClosestHit/Miss + SER）与 `VK_KHR_ray_query`** 的立方体素专用硬件全局光照方案，目标是实现**实时多 Bounce 漫反射 + 镜面反射 + 任意自发光方块软阴影**的电影级体素渲染。

---

## 1. 为什么在体素上跑 RT Core（RTX）极具优势

1. **Greedy Meshing 极简几何**：
   - 连续同材质方块通过 Greedy Meshing 合并后，一个 `64×64×64` Chunk 的三角形数量通常比原始方块面少 **10×–30×**，RT Core 的 BVH 遍历极其轻量，可轻松承载全动态 Chunk TLAS 增量重建。
2. **天然正交无 UV 接缝**：
   - 命中任意三角形后，只需 `floor(hit_pos - hit_normal * eps)` 即可唯一确定体素坐标与朝向 `face_id ∈ 0..5`，**完全不需要 Mesh Cards、UV 展开或复杂几何参数化**。
3. **自发光方块（Emissive Voxels）直接参与光追**：
   - 岩浆、灯笼、霓虹方块无需转换成近似点光源，直接作为多边形自发光表面被 RT Core 命中并产生真实软阴影与彩色溢色。

---

## 2. 加速结构与 RT Core 管线选型

### 2.1 两级加速结构（BLAS / TLAS）
- **Chunk 级 BLAS**：每个非空 Chunk（`32^3` 或 `64^3`）维护独立 Greedy-Meshed BLAS，启用 `ALLOW_UPDATE` / 异步压缩队列；玩家拆建方块时仅重建 1–4 个受影响 Chunk 的 BLAS。
- **动态物体 BLAS**：独立实体（门、载具、掉落物）挂入同一 TLAS，与静态 Chunk 统一参与间接光遮挡与反射。
- **不透明与 Alpha-Test 拆分**：全实心方块走 `OPAQUE` 跳过 AnyHit；树叶、玻璃、栅栏等走独立实例掩码或 `VK_EXT_opacity_micromap`（OMM）加速。

### 2.2 `RayTracingPipeline` vs `RayQuery` 分工
- **主 GI / 镜面多 Bounce（`VK_KHR_ray_tracing_pipeline`）**：
  - 使用独立 **RayGen → ClosestHit / Miss** 管线，配合 **SER（Shader Execution Reordering，可用时）** 按命中材质/方向对射线重排序，最大化 RT Core 与 warp 利用率。
- **直接光软阴影 / AO（`RayQuery` inline）**：
  - 仅需可见性判定（`RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH | RAY_FLAG_SKIP_CLOSEST_HIT_SHADER`），在计算着色器内联完成，避免 Shader Table 调度开销。

---

## 3. 多 Bounce 光照架构：ReSTIR GI + 体素面缓存（Voxel Surface Cache）

结合 **1-Bounce 精准路径采样** 与 **体素面缓存的多 Bounce 复用**：

1. **Voxel Surface Cache（世界空间永久/半永久多 Bounce 底座）**：
   - 类似 [sharc.rs](file:///root/prisma/crates/prism-render/src/sharc.rs) 的空间哈希思想，但键直接简化为 **`(voxel_xyz, face_id)`**（64-bit 精确整数键，无浮点量化漂移）。
   - 每次 RT Core 射线在次级命中点（Secondary Hit）查表读取该体素面的历史出射辐射度 $L_o$，从而以 **1–2 次硬件射线深度** 稳定实现 **3–4+ Bounce** 的漫反射能量传递。
2. **屏幕空间 ReSTIR GI（1 spp 主路径重采样）**：
   - 对全分辨率 G-Buffer（或半分辨率 + 上采样）发射 **1 条 cosine-weighted / NEE（Next Event Estimation）射线**；
   - 通过 **时空重采样（Temporal + Spatial Reservoir Reuse）** 把有效采样数放大到等效 8–16 spp，精准捕捉自发光方块与强太阳光反弹的近景高频间接光。
   - Reservoir 重用时以 **体素面法线点积 + 深度平面距离** 为门控，严防跨方块棱角与薄墙错误重用。
3. **镜面反射独立通道（Specular RT）**：
   - 对光滑金属/水/冰方块，从 G-Buffer 按 GGX 重要性采样发射 1 条镜面射线，命中后直接读取命中点直接光 + Voxel Surface Cache，实现清晰且物理正确的实时倒影。

---

## 4. 降噪与 DLSS-RR / NRD 联动

依据 [rtx-dlss-integration-design.md](file:///root/prisma/docs/rtx-dlss-integration-design.md) 的 Guide Buffer 契约：

- **标准路径（NRD / 自研时空降噪）**：
  - 输出 `DiffuseHitT + SpecularHitT`、屏幕空间 Motion Vector、世界法线/粗糙度，分别执行 Diffuse/Specular 双通道时空降噪（保留方块锐利棱角，避免糊掉直角边缘）。
- **NVIDIA RTX 旗舰路径（DLSS-RR 光线重构）**：
  - 将原始 1 spp Diffuse/Specular Radiance 与 Guide Buffers（Albedo、Specular Albedo、Normal+Roughness、Motion Vectors、Depth）直接喂入 DLSS-RR，同时完成超分与无鬼影去噪，**彻底替代传统分离式时空降噪器**。

---

## 5. 与软件方案（`voxel-software-gi-design.md`）的统一接口

两套立方体素 GI 共享上层 Chunk 数据与 G-Buffer 合成接口：
- **桌面 RTX 设备**：走本文件 `RT Core Pipeline + Voxel Surface Cache + ReSTIR GI (+ DLSS-RR)`；
- **移动端 / 无硬件光追设备**：自动回退到 [voxel-software-gi-design.md](file:///root/prisma/docs/voxel-software-gi-design.md) 的 `64 叉树位掩码 DDA + 体素面缓存 + 3D 辐射级联`。
