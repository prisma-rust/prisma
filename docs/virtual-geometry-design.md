# 多平台虚拟几何体架构设计（Beyond-Nanite Cluster-DAG / 软硬混合光栅 / 光追同源）

> **层级**：L2 专项子系统设计契约 · 渲染进阶
> **状态**：M1–M3（含 `MeshCooker` 接入）与 M5/M6 核心数学及运行时已落地（`prism-asset` / `prism-cooker` / `prism-render`）；M4 离线 Slang 着色器待 `slangc` 宿主环境编译签入
> **关联**：[DESIGN.md](DESIGN.md)、[rtx-dlss-integration-design.md](rtx-dlss-integration-design.md)、[prt-gi-64tree-design.md](prt-gi-64tree-design.md)、[mobile-raytracing-gi-design.md](mobile-raytracing-gi-design.md)

本文定义 Prisma 引擎面向**最新桌面旗舰 GPU（NVIDIA RTX 40/50、AMD RDNA 3.5/4）与移动旗舰 SoC（骁龙 8 Gen 3 / 8 Elite Adreno 750/850+、天玑 9400 Immortalis-G925）** 的统一虚拟几何体（Virtual Geometry）渲染架构。在继承传统 Nanite「Cluster-DAG 无级自动 LOD + VisBuffer 零 Quad Overdraw」优势的同时，重点攻克传统方案在**硬件光追几何不同源、动态骨骼蒙皮、高频植被退化、离线包体膨胀及移动端 TBDR 带宽墙**五大领域的结构性短板。

---

## 0. 设计定位与多平台硬件分级矩阵

按照 [`DESIGN.md`](DESIGN.md) §2.1–§2.3 的铁律，全平台仅维护**一套虚拟几何体 RenderGraph 拓扑与数据格式**，严禁编写 `#[cfg(target_os)]` 渲染分支。差异完全收敛到 `crates/prism-render/src/capabilities.rs` 在初始化时探测出的 `VirtualGeometryCaps` 与 `VirtualGeometryTier`：

| 分级 | 枚举标识 | 目标硬件代表 | 探测到的核心扩展与特性 | 管线执行路径 |
| :--- | :--- | :--- | :--- | :--- |
| **Tier 0** | `NextGenClasMeshSw` | RTX 40/50 Blackwell、RX 9000 | `VK_EXT_mesh_shader` + `shaderBufferInt64Atomics` + `VK_NV_cluster_acceleration_structure` (CLAS) + `VK_NV_displacement_micromap` (DMM) + `VK_EXT_opacity_micromap` (OMM) | **Subgroup DAG 裁剪 → Task/Mesh Shader（$\ge 4\text{ px}^2$）+ 64-bit Atomic 软光栅（$< 4\text{ px}^2$）→ CLAS 硬件簇级光追同源 → DLSS-RR / DLSS-G** |
| **Tier 1A** | `FlagshipMeshSw` | 骁龙 8 Gen 3 / 8 Elite (Adreno 750/850+)、天玑 9400 (Immortalis-G925)、主流独显 | `VK_EXT_mesh_shader` + `shaderBufferInt64Atomics` + Subgroup + `VK_KHR_ray_query`（无 CLAS） | **Subgroup DAG 裁剪 → Task/Mesh Shader + 64-bit 软光栅混合 → Active-Cut 动态簇 BLAS 池 → Adreno ANF / NRD / SGSR2** |
| **Tier 1B** | `TileLocalSubpass` | 支持 Tile Image / 强 TBDR 的现代移动 SoC（无 64-bit Atomics 或优先片上带宽时） | `VK_EXT_shader_tile_image` 或 Transient Subpass + `VK_EXT_mesh_shader` / `drawIndirectCount` | **同一 DAG 裁剪 → `TRANSIENT_ATTACHMENT` VisBuffer (Subpass 0) → 片上 GMEM/TLS 解析着色 (Subpass 1，零主存中间带宽) → ANF / TAA** |
| **Tier 2** | `IndirectIndexedFallback` | 主流 Vulkan 1.2 设备、核显、CI `llvmpipe` | `bufferDeviceAddress` + `descriptorIndexing` + `drawIndirectCount` | **同一 DAG 裁剪 → `vkCmdDrawIndexedIndirectCount` → 32-bit VisBuffer → Analytic Resolve → TAA** |

> **零兼容纪律**：按照 [`versioning.md`](versioning.md) 规范，`.rcld` 格式与运行时结构从第一天起采用单一最新版本，不保留历史版本兼容分支或退化垫片。

---

## 1. 数据契约：`.rcld`（`RCLD` v1 64 KB 分页虚拟几何体）

`.rcld` 是纯小端序二进制格式，字节规范由 `crates/prism-asset/src/formats_virtual_geo.rs` 定义并由 `crates/prism-cooker/src/cooker/virtual_geo.rs` 生成。运行时包体通过 `.pak` 的 `FLAG_STREAMED`（见 [`DESIGN.md`](DESIGN.md) §10.6）支持按页 `pread` 流式加载，零 CPU 解码重排。

### 1.1 整体布局与常量约束

| 常量 | 数值 | 说明 |
|---|---:|---|
| `RCLD_MAGIC` | `b"RCLD"` | 4 字节魔数 |
| `RCLD_VERSION` | `1` | 格式版本号 |
| `RCLD_PAGE_SIZE` | `65536` (64 KiB) | 单页上限，与显存子分配页及 OS 页对齐友好 |
| `RCLD_MAX_CLUSTER_VERTICES` | `128` | 单 Cluster 顶点数硬上限 |
| `RCLD_MAX_CLUSTER_TRIANGLES` | `128` | 单 Cluster 三角形数硬上限（局部索引 `u8x3`，VisBuffer 的 `TriID` 占 7 bit） |
| `RCLD_MAX_CLUSTER_BONES` | `8` | 单 Cluster 局部骨骼调色板上限 |

文件物理布局：

```text
[ RcldHeader: 64 bytes ]
[ PageDescriptorTable: page_count × 20 bytes ]
[ ClusterGroupTable: group_count × 48 bytes ]
[ Page Payloads (Page 0 Resident Root | Page 1..N Streamed Child Pages) ]
```

### 1.2 `RcldHeader`（64 字节）

| 偏移 | 大小 | 字段 | 说明 |
|---:|---:|---|---|
| 0 | 4 | `magic` | `b"RCLD"` |
| 4 | 1 | `version` | `1` |
| 5 | 1 | `flags` | bit0: `SKINNED`；bit1: `HAS_DMM`；bit2: `HAS_SPLATS`；bit3: `GEOMORPHING` |
| 6 | 2 | `max_lod_level` | DAG 最大层级数 |
| 8 | 4 | `page_count` | 总页数（Page 0 恒为常驻 Root Page） |
| 12 | 4 | `group_count` | `RcldClusterGroup` 总数 |
| 16 | 4 | `cluster_count` | `RcldCluster` 总数 |
| 20 | 4 | `root_group_index` | 最粗根 Group 下标 |
| 24 | 12 | `bounds_min` | `[f32; 3]` 资产局部空间最小包围盒 |
| 36 | 12 | `bounds_max` | `[f32; 3]` 资产局部空间最大包围盒 |
| 48 | 4 | `page_table_offset` | Page 表字节偏移（固定 `64`） |
| 52 | 4 | `group_table_offset` | Group 表字节偏移 |
| 56 | 4 | `payload_offset` | Page 载荷起始偏移 |
| 60 | 4 | `reserved` | 置零对齐到 64 字节 |

### 1.3 分页描述符（`RcldPageDescriptor`）与集群组（`RcldClusterGroup`）

1. **`RcldPageDescriptor`（20 字节）**：
   - `page_index (u32)`、`byte_offset (u32)`、`byte_size (u32)`、`first_cluster (u32)`、`cluster_count (u16)`、`lod_level (u8)`、`flags (u8)`（bit0 = `RCLD_PAGE_FLAG_RESIDENT_ROOT`）。
   - Page 0 随资产挂载立即上传至 GPU `PagePool` 并永久钉扎（Pinned），保证任何帧在高精度页尚在流式传输时始终存在可用几何保底，**彻底杜绝破洞**。
2. **`RcldClusterGroup`（48 字节）**：
   - `group_id (u32)`、`lod_level (u8)`、`primitive_kind (u8)`、`child_group_count (u16)`、`page_index (u32)`、`child_group_first (u32)`、`first_cluster (u32)`、`cluster_count (u16)`、`reserved (u16)`、`parent_error (f32)`、`self_error (f32)`、`bounding_sphere ([f32; 4])`（xyz = center, w = radius）。
   - **硬不变量**：对任意非根 Group，恒有 `parent_error > self_error` 且所有同组 Cluster 共享完全相同的外边界顶点集合，保证任意 DAG Cut 切口下 **100% 零拓扑裂缝（Crack-Free）**。

### 1.4 三模态 Cluster 载荷与 20 字节极致量化顶点

每个 `RcldCluster` 根据 `primitive_kind` 承载三种前沿几何原语：

| `VirtualGeoPrimitiveKind` | 适用场景 | 载荷内容 |
|---|---|---|
| `Triangles (0)` | 通用刚体/骨骼表面 | `QuantizedClusterVertex[N]`（20 B/顶点）+ `u8[3][M]` 局部三角形索引 + `normal_cone ([f32; 4])` + 8-Bone 调色板 |
| `DisplacedMicroMesh (1)` | 扫描级高精度表面、动态形变地形 | 基础控制三角形（$\le 64$）+ `i16` 11-bit 标量微位移阵列，运行时由 Mesh Shader 按屏幕边长执行 $4^k$ 重心细分（或直通 `VK_NV_displacement_micromap`），包体再降 $6\times \sim 10\times$ |
| `SggxGaussianSplats (2)` | 中远景植被树冠、草海、毛发 | 各向异性 3D 高斯微面元（`center[3]`, `scale[3]`, `rotation[4]`, 6 分量 `sggx_cov[6]`, `opacity`），从近景 OMM 镂空三角形无缝切换为 SGGX 微薄片体散射 |

**20 字节 `QuantizedClusterVertex` 布局**（较传统 `Vertex`（`crates/prism-render/src/mesh.rs`）60 字节压缩 **66.7%**）：

| 字段 | 类型 | 字节 | 说明 |
|---|---|---:|---|
| `pos_q` | `[u16; 3]` | 6 | 相对 Cluster 局部 AABB 的 16-bit 归一化位置 |
| `normal_oct` | `[i8; 2]` | 2 | 八面体（Octahedral）单位法线量化 |
| `tangent_angle` | `i8` | 1 | 切线帧旋转角（符号位编码手性 handedness） |
| `bone_local_pair` | `u8` | 1 | 局部调色板主/次骨骼索引打包（`hi4 \| lo4`） |
| `uv_f16` | `[u16; 2]` | 4 | IEEE-754 half-float UV0 |
| `geomorph_delta_q` | `[i16; 3]` | 6 | 指向父级简化表面目标位置的连续 Geomorphing 残差向量 $\Delta \mathbf{p}$ |

---

## 2. 离线构建管线（`prism-cooker::cooker::virtual_geo`）

离线构建严格驻留在构建期 crate `crates/prism-cooker`（`crates/prism-cooker/src/cooker/mod.rs`）内，受 `CookProfile.mesh.virtual_geometry`（`crates/prism-cooker/src/cooker/profile.rs`）开关控制，**绝不进入 `prism-app` 玩家依赖树**。

1. **Morton 3D 空间重排与 128V/128T Meshlet 切分**：按三角形质心 Morton Z-order 排序，贪心聚拢共享顶点的三角形至 128 顶点 / 128 三角形上限，计算局部 AABB、包围球、法线锥（`axis.xyz + sin_half_angle`）与 8 根局部骨骼调色板。
2. **共享边图聚类（Cluster Grouping）**：依据簇间共享边权重将 4–8 个同级 Cluster 聚为单个 `RcldClusterGroup`。
3. **锁边界 QEM 简化与连续 Geomorphing 编码**：
   - 检测落在 Group 外边界（边仅出现于组内单侧，或位于原始模型外边界）的顶点并强制加锁（`locked = true`）；
   - 仅对 Group 内部边执行 Garland–Heckbert 二次误差度量（QEM）边坍缩，目标将三角形数减半（$\approx 50\%$）；
   - 记录每个被坍缩顶点 $v$ 的最终目标位置 $\mathbf{p}_{\text{target}}$，写出 $\Delta \mathbf{p} = \mathbf{p}_{\text{target}} - \mathbf{p}_v$ 至 `geomorph_delta_q`；
   - 强制单调误差递增：$\varepsilon_{\text{parent}} = \max(\varepsilon_{\text{qem}}, \max_{c \in \text{children}} \varepsilon_c + 10^{-4})$。
4. **跨模态代理生成与 64 KB 分页打包**：当启用 `alpha_masked_foliage` 且进入高层粗 LOD 时自动拟合 `SggxSplatPrimitive`；随后按层级与空间局部性切分写入 `RCLD_PAGE_SIZE`（64 KiB）物理页。

---

## 3. 运行时 GPU 驱动管线（`prism-render::virtual_geo`）

### 3.1 `SceneScope` 64 KB Page Pool 与光线驱动流式

- 复用 `crates/prism-render/src/suballocator.rs` 管理固定预算的全局 `PagePool`（桌面默认 256 MiB = 4096 页，移动默认 64 MiB = 1024 页）。
- **零破洞缺页回退（Seamless Parent Fallback）**：当 GPU/CPU 遍历 DAG 需要访问未驻留的子页时，立即向 `PageFeedbackRing` 提交缺页请求（区分光栅高优 `RasterCut` 与光线追踪反射请求 `RayTracingHit { cone_width }`），同时本帧自动渲染已常驻的父级 Group。
- **LRU 淘汰**：Page 0 永久锁定，非根页按 `last_used_frame` 执行 O(1)/LRU 淘汰并经 `StagingArena` 异步上载新页。

### 3.2 超分感知的 DAG 切口方程与双阶段 HZB 剔除

1. **超分感知误差阈值（Upscaler-Aware LOD Bias）**：
   结合当前渲染模式与 [rtx-dlss-integration-design.md](rtx-dlss-integration-design.md)、`crates/prism-render/src/anf.rs`、`crates/prism-render/src/mobile_upscaler.rs` 的内部分辨率缩放比，动态收紧屏幕误差阈值 $\tau_{\text{effective}}$：
   $$\tau_{\text{effective}} = \max\left(0.25,\ \tau_{\text{base}} \cdot \frac{H_{\text{internal}}}{H_{\text{output}}} \cdot 2^{\text{lod\_bias}}\right)$$
   保证开启 DLSS Performance / Adreno ANF / SGSR2 后，重构出的高频几何边缘依然达到原生 4K / QHD 锐度。
2. **连续 Geomorphing 混合因子**：
   $$\alpha = \text{saturate}\left(\frac{\varepsilon_{\text{screen}} - \tau_{\text{effective}}}{\tau_{\text{effective}} \cdot \text{morph\_window}}\right), \quad \mathbf{p}_{\text{morph}} = \mathbf{p}_{\text{self}} + \alpha \cdot \Delta \mathbf{p}$$
3. **四重级联剔除与 Two-Phase HZB**：
   - 视锥剔除（沿用 `crates/prism-render/src/culling.rs` 右手系 Vulkan $[0, 1]$ 深度约定）→ 法线锥背面剔除 → 亚像素微簇剔除 → **双阶段 HZB 遮挡剔除**（Phase 1 用 Prev-HZB 剔除，被挡候选入队；Phase 1 光栅后生成 Curr-HZB，Phase 2 复测并补画新露出簇，零闪烁、零漏画）。

### 3.3 软硬混合光栅与解析重心导数 VisBuffer

- **自动面积分流**：
  - 平均投影面积 $\ge 4.0\text{ px}^2$ 的 Triangle Cluster：路由至 **Task/Mesh Shader**（Tier 0/1）或 `vkCmdDrawIndexedIndirectCount`（Tier 2），写出 `32-bit VisBuffer = (visible_cluster_idx << 7) | tri_idx`；
  - 平均投影面积 $< 4.0\text{ px}^2$ 的亚像素微三角形簇：在支持 `shaderBufferInt64Atomics` 的设备上路由至 **Compute 64-bit 软光栅**，通过 `InterlockedMax` 原子写入 `64-bit VisBuffer = (reverse_z_depth_bits << 32) | vis32`；
  - `SggxGaussianSplats` 簇：路由至专用 Splatting 队列，执行椭圆投影与 SGGX 各向异性透射着色。
- **解析导数重构（零 $2\times 2$ Quad Overdraw）**：
  `AnalyticBarycentrics` 对屏幕任意像素直接求取透视矫正重心权重 $(\lambda_0, \lambda_1, \lambda_2)$ 与解析屏幕偏导数 $(\partial \lambda / \partial x, \partial \lambda / \partial y)$，进而无须依赖硬件像素 Quad 即可精确算出纹理梯度 `ddx(uv)` / `ddy(uv)`，喂给 `crates/prism-render/src/bindless.rs` 的 `SampleGrad` 与 `assets/shaders/slang/scene_frag.slang` PBR 着色管线。
- **Geomorphing 运动矢量消抖**：
  解析阶段显式计算上一帧与当前帧的 Geomorphing 混合位置差，将其从屏幕空间运动矢量中剔除后写入 `FORWARD_VELOCITY_H` / `PT_VELOCITY_H`，确保 **DLSS-RR、Adreno ANF 与 TAA** 在连续 LOD 过渡期间零鬼影、零拖影。

---

## 4. 光栅/光追几何同源与神经超分接口契约

1. **Active-Cut 光追同源（Unified Cluster-BLAS / CLAS）**：
   - GPU DAG 裁剪产出的 `ActiveCutClusterList` 同时作为光追输入源：在 Tier 0 上通过 `VK_NV_cluster_acceleration_structure` 直接汇编硬件 CLAS；在 Tier 1 上增量维护 Active-Cut Cluster BLAS 池并挂入主 TLAS，让 `crates/prism-render/src/pt_pass.rs`、`crates/prism-render/src/restir_gi.rs` 与 `crates/prism-render/src/sharc.rs` 与主视角高模完全对齐，消除自阴影断裂与低模反射穿帮。
2. **Guide Buffer 契约对齐**：
   - VisBuffer Resolve Pass 按 [rtx-dlss-integration-design.md](rtx-dlss-integration-design.md) §3 语义产出全套 Guide Buffers（`FORWARD_DEPTH_H` / `FORWARD_NORMAL_H` / `FORWARD_VELOCITY_H` 及对应 `PT_*` 句柄），无需额外的全分辨率预渲染 Pass。

---

## 5. 分阶段实施里程碑与验收契约（M1–M6）

| 里程碑 | 状态 | 范围与核心交付 | 关键文件 | 量化验收门禁（DoD） |
|---|---|---|---|---|
| **M1** | ✅ 已落地 | **离线分页格式 `RCLD` 与锁边界 Cluster-DAG Cooker** | `crates/prism-asset/src/formats_virtual_geo.rs`<br>`crates/prism-cooker/src/cooker/virtual_geo.rs`<br>`crates/prism-cooker/src/cooker/profile.rs`<br>`crates/prism-cooker/src/cooker/mod.rs` | 单顶点量化 $\le 20\text{ B}$；任意混合 LOD Cut 相邻 Group 边界 **100% 零裂缝**；`parent_error > self_error` 严格单调；`MeshCooker` 接线并保持玩家树 `check_deps.py` 零新增依赖。 |
| **M2** | ✅ 已落地 | **能力分级探测、64 KB Page Pool LRU 流式器与 DAG Cut 裁剪** | `crates/prism-render/src/capabilities.rs`<br>`crates/prism-render/src/virtual_geo.rs`<br>`crates/prism-render/src/render_graph/settings.rs`<br>`crates/prism-asset/src/package/mod.rs`<br>`crates/prism-asset/src/runtime/`<br>`crates/prism-engine/src/asset_resolver/decoders.rs` | 覆盖 Tier 0/1A/1B/2 全探测；`PackageReader`/`ResourceManager::read_asset_range` 按字节区间 O(1) 流式加载 64 KB Page；`VirtualGeoRuntime` + `RenderSettings` + `GpuAssetResolver` RCLD 回退/流式桥接全部单测通过。 |
| **M3** | ✅ 已落地 | **VisBuffer 编码、解析重心导数与 Geomorphing 消抖速度场** | `crates/prism-render/src/virtual_geo.rs` | 32-bit/64-bit VisBuffer Reverse-Z 单调排序正确；解析 `ddx/ddy` 与有限差分屏幕梯度误差 $< 10^{-4}$；Geomorphing 速度补偿与超分 LOD 偏置单测全绿。 |
| **M4** | ⏳ 待 `slangc` 宿主 | **GPU Slang 着色器与 `VirtualGeoPass` RenderGraph 接入** | `assets/shaders/slang/vg_*.slang`<br>`crates/prism-render/src/vg_pass.rs`<br>`assets/shaders/budgets.json` | 通过 `xtask shader-bindgen` 与 `shader-budget`；Task/Mesh Shader + Compute 64-bit 软光栅 + Tile-Local Subpass 在 RenderGraph 端到端出图。 |
| **M5** | 🟡 核心已落地 / 待真机渲染接线 | **光栅/光追几何同源（Active-Cut RT 增量池与光线驱动流式）** | `crates/prism-render/src/virtual_geo.rs`<br>`crates/prism-render/src/acceleration_structure.rs`<br>`crates/prism-render/src/pt_pass.rs`<br>`crates/prism-render/src/restir_gi.rs` | `ActiveCutRtClusterPool` 增量 diff 与 `evaluate_ray_guided_page_fault` 已落地并单测通过；待 M4 着色器就绪后接入 TLAS / CLAS 与 DLSS-RR / ANF 联合真机 SSIM 验收（$\ge 0.995$）。 |
| **M6** | 🟡 核心数学与结构已落地 / 待着色器接线 | **全模态扩展：8-Bone 簇级延迟蒙皮、DMM 微细分、植被 SGGX Splatting** | `crates/prism-render/src/virtual_geo.rs`<br>`crates/prism-cooker/src/cooker/virtual_geo.rs`<br>`assets/shaders/slang/vg_splat_sggx.slang` | `evaluate_cluster_bone_envelope`/`skin_cluster_vertex`、`subdivide_dmm_triangle`（$4^k$ 细分）、`evaluate_sggx_ndf`/`transmittance` 已落地并单测通过；待 M4 GPU 管线接线后全场景性能压测。 |
