# PRT GI 与 64 叉树流式设计（M1–M4 契约）

> **层级**：L2 专项子系统设计契约 · 渲染进阶
> **状态**：设计稿（尚未落地代码）
> **关联**：[DESIGN.md](DESIGN.md) §6（Baked GI 与 Render Graph）、[mobile-raytracing-gi-design.md](mobile-raytracing-gi-design.md)、[lessons-learned.md](lessons-learned.md)

本文把 M1–M4 四个里程碑的交付物固化为**可验收的工程契约**：数据格式、管线拓扑、GPU 侧数值约定，
以及每条量化指标的**测量方法与判定门限**。任何实现偏离本文件的契约，都以本文件为准先行修订。

> **质量优先基线（最高约束）**：本阶段不采用任何以降采样、降精度、时间抽取、按可见性裁剪
> 换取性能的手段。所有指标都在**全分辨率、f32 运算、1:1 采样**下判定。
> 性能不达标时改算法或改表示，不改画质 —— 具体红线见 §7.3。

---

## 0. 现状与差距（先认清地基）

| 能力 | 仓库现状 | 本设计需要 |
|---|---|---|
| 离线烘焙 | `prism-build-pipeline bake-gi`（`gi-bake` feature）：RayQuery 多 bounce 路径追踪，投影到 **2 阶 SH（9 系数）** 规则网格探针 | 高阶 PRT 传输向量 + **PT 聚类** + **64 叉树** + `.prtbin` 导出 |
| 运行时表示 | `prism-render::gi`（`ProbeVolumeInfo` + `eval_sh9`）、`probe_loader`（`PRPV` 魔数） | PRT 系数空间的 cluster-major 稀疏存储与求值 |
| 运行时消费 | `scene_frag.slang` 的 `PBR_FLAG_GI` 分支，3D 纹理 `Load` + 手动三线性 | 新的 `EvaluatePRT64` 采样层与 Pass 4 合成 |
| 动态光源 | ReSTIR GI（光栅二次射线）、SHARC 缓存；**无** cluster 化动态点光传播 | Cluster-Major 传播 + 定点原子累加 |
| 空间结构 | 规则网格（`dims: [u32;3]` + `spacing`） | 64 叉树分块 + 异步流式 |

结论：**本设计不是既有 probe volume 的调参，而是一条新增的表示与管线**。既有 `PRPV` 2 阶网格路径
保留为低频 diffuse 回退信号源，PRT 路径以 `gi_mode` 的新取值接入，二者共享 `SceneScope` 与
`ForwardPass` 合成公式（同一 `E/π` 量纲），**不复制两套求值代码**。

---

## 1. 术语裁决（歧义必须先消除）

| 术语 | 本文件定义 | 明确**不是** |
|---|---|---|
| **PRT64** | 7 阶实球谐，共 `(7+1)^2 = 64` 个系数 / 通道的**预计算辐射度传输**向量 | 64 条采样光线；64 个探针 |
| **64 叉树** | 每节点按 `4×4×4` 细分为 **64 个子节点**的空间索引树（八叉树的 64 分版本） | 64 层树；八叉树的笔误 |
| **聚类（PT 聚类工具）** | 离线阶段对探针传输矩阵做 PCA + k-means，得到 **cluster**：一组共享同一 64 系数传输基的 64 树叶节点 | 运行时 k-means；材质聚类 |
| **`EvaluatePRT64`** | 「按当前光照把 cluster 的 64 系数求值成辐射度 / 辐照度」的**整条链路**（Pass 1–4），不是单次函数调用 | 每像素 64 次点积（见 §7.2，读放大与频域双重理由） |
| **双 `uint32` 位运算** | 稀疏键 / 掩码 / f16 对在 GPU 上**统一**表示为 `uvec2`，从第一天起就是唯一表示 | 硬件缺 64 位时的「兼容层」（违反 [versioning.md](versioning.md) 零兼容纪律，禁止） |

> **零兼容纪律**：`uvec2`、定点数累加、f16 打包都是**唯一实现**，不存在「检测硬件走另一条路」的分支。
> 设备能力差异**不影响**数学路径、分辨率或精度：本阶段不做能力驱动的画质降级，
> 能力探测只用于判断「能否运行」，不用于决定「运行得多准」（与 §7.3 质量红线一致）。

---

## 2. 数据契约：`.prtbin`

单一文件、小端序、16 字节对齐，可直接 `vkCmdCopyBufferToImage` / `Buffer` 上传，**无运行时重排**。

### 2.1 头部（`PRTB`，64 字节）

| 偏移 | 大小 | 字段 | 说明 |
|---:|---:|---|---|
| 0 | 4 | `magic` | `b"PRTB"` |
| 4 | 2 | `version` | `u16`，当前 `1` |
| 6 | 2 | `flags` | bit0：body 为 f16（未置位则为 f32）；bit1：含 `valid` 字节 |
| 8 | 4 | `sh_band` | `u32`，恒为 `7`（64 系数） |
| 12 | 4 | `leaf_count` | `u32`，**硬上限 8600**（见 §2.4） |
| 16 | 4 | `node_count` | `u32` |
| 20 | 4 | `cluster_cells` | `u32`，每 cluster 的空间格数（默认 1） |
| 24 | 12 | `world_origin` | `[f32;3]`，根节点 AABB 最小角 |
| 36 | 4 | `world_extent` | `f32`，根节点立方体边长（米） |
| 40 | 4 | `leaf_grid` | `u32`，叶子边长 = `world_extent / 4^leaf_grid` |
| 44 | 4 | `payload_bytes` | `u32`，cluster 载荷字节数 |
| 48 | 8 | `reserved` | 置零 |
| 56 | 8 | `built_from_sha` | 场景 `scenes.pak` 前 8 字节 SHA-256 截断，用于拒绝错配 |

### 2.2 节点表（`node_count × 16` 字节）

| 偏移 | 大小 | 字段 |
|---:|---:|---|
| 0 | 4 | `first_child`（`u32`；`0xFFFF_FFFF` 表示叶子） |
| 4 | 4 | `leaf_index`（`u32`；内部节点为 `0xFFFF_FFFF`） |
| 8 | 8 | `child_mask`（`u64`，bit `i` 表示子节点 `i` 存在；**GPU 侧为 `uvec2`**） |

节点顺序为 BFS（广度优先），因此 `first_child` 单调递增，可做一次线性扫描完成流式插入。
GPU 端节点表与 cluster 载荷**分两个 buffer**：节点表常驻，cluster 载荷按区块可卸载。

### 2.3 Cluster 载荷（cluster-major）

`leaf_count` 条记录，每条：
`64 系数 × RGB × 2 字节（f16） = 384 字节`，可选尾随 `1` 字节 `valid` 标志（`flags.bit1`）。
排列为 `[coeff][rgb]`，与 `gi.slang` 的 `EvalSH9Radiance` 系数序一致（基函数编号 0..63，
编号规则沿用 `gi.rs` 文档表中的实球谐顺序扩展到 7 阶）。

**为什么是 cluster-major**：运行时 Pass 1 以 `workgroup = 1 cluster` 调度，一次合并读入
384 字节（= 96 个 `uint32`，按 `uvec2` 双字处理），workgroup 内 64 线程各持有 1 个系数，
避免任何跨 lane 的 gather。这是 M3「Cluster-Major 传播」的存储前提。

### 2.4 体积预算（M1 验收的直接推导）

```
cluster 载荷 = leaf_count × 384 B
节点表      = node_count × 16 B   ≈ leaf_count × 16 B × 1.02（64 叉树内部节点仅约 1.6%）
头部        = 64 B
```

由上式可直接解出体积上限对应的叶数：`leaf_count` 超过 `8600` 必定超出 3.5 MB 预算，
**烘焙工具必须在该阈值处失败退出**，而不是静默截断。这是一个只依赖格式常量的硬约束，
与场景复杂度无关 —— 场景复杂度决定的是能否在阈值内完成烘焙，而不是阈值本身。

---

## 3. M1：离线烘焙工具链

新增子命令 `prism-build-pipeline bake-prt`（沿用 `gi-bake` feature 与 `bake_gi.rs` 的 BLAS / TLAS、
场景加载、`collect_bake_instances` 全部基础设施，不重复实现）。

### 3.1 四阶段

1. **采样**：按 64 树叶子候选中心（先按 `leaf_grid` 规则切分）发射方向采样，
   每条样本记录「方向 × 命中点 × 后续反弹」的完整传输，投影到 64 系数基。
   样本数从 `bake_samples_per_leaf`（默认 `4096`）起，按方差自适应加采样（目标每个系数相对误差 ≤ 3%）。
2. **PT 聚类**：对 `leaf_count × 64×3` 传输矩阵做 PCA，取主成分（累计能量 ≥ 99%）后 k-means，
   `k = leaf_count`。聚类是**质量优化**：让同簇叶子共享传输基，同时把「墙两侧不应插值」的叶子分到不同簇，
   这是 §5.3 防漏光的第一道机制。
3. **64 树构建**：自底向上，`4×4×4` 合并；叶子的分裂判据为
   （a）AABB 跨越几何面（占用栅格两侧都有面），（b）传输向量间夹角超过 `cluster_merge_deg`（默认 12°）。
   判据（a）保证**没有任何叶子跨越墙体**。
4. **导出**：`.prtbin`（§2）+ 烘焙报告 JSON（耗时、命中率、每簇残差、`leaf_count`）。

### 3.2 验收

| 项 | 门限 | 测量 |
|---|---|---|
| 端到端耗时 | **≤ 15 min**（单卡 RTX 4090） | `bake-prt` 自带 wall-clock 报表，不含首次 `scenes.pak` 构建 |
| 输出体积 | **≤ 3.5 MB** | `leaf_count ≤ 8600` 硬断言 + 文件实际字节数 |
| 场景规模 | 500 m × 500 m 城市，≥ 3000 静态实例 | 报告内记录实例 / 三角面 / 叶数 |

> **首要风险是显存而非时间**：500 m 城市的 BLAS 占用随三角面数线性增长，大场景下显存可能先于
> 15 min 上限成为约束。本文件不预设耗时或显存的具体数值，以工具自带计时与实测显存为准；
> 三角面预算确认后（§7.5）再决定 BLAS 是否需要分块构建。

---

## 4. M2：运行时基础管线

### 4.1 Pass 拓扑（插入现有 Render Graph）

```
ShadowMapPass → ForwardPass → GtaoPass → RestirGiPass → PrtPass(1,2,3) → PostPass
                                                   ↘ ForwardPass 下一帧采样 PrtPass 输出（1-frame latency）
```

与 `GtaoPass` / `RestirGiPass` 同构：**帧级双缓冲 + 1 帧延迟**，资源归 `FrameScope`，
绝不复用 `SceneScope` 的 `vk::Image`（DESIGN §6.1 红线）。`.prtbin` 的节点表与 cluster 载荷归
`SceneScope`，只在换场时销毁。

| Pass | 类型 | 工作 | 输出 |
|---|---|---|---|
| **1. ClusterEval** | compute | 对**全部** cluster 求值 64 系数 × 当前太阳 / 天光朝向 | `cluster_radiance`（f16 存储，f32 运算） |
| **2. Propagate** | compute | 动态光源贡献的 6 邻域传播（M3） | `cluster_radiance` 增量（定点原子累加） |
| **3. ResolveVolume** | compute | 簇 → 稀疏体素网格（**64³**，8 m）三线性泼溅 + 泄漏门控，输出 2 阶（9 系数）辐照度 | `gi_volume`（f32，64³ × 9×3） |
| **4. PixelSample** | pixel | `EvaluatePRT64`：**全分辨率**三线性采样 + `E/π` 合成 | HDR 颜色 |

Pass 3 降到 2 阶是**表示层的正确分层**，不是降质量：辐照度天然低频，高阶能量在
cluster → 体素的 resolve 积分中已经完成；像素层再引入 64 阶只是重复积分同一份能量。
体素层与像素层都是**全分辨率、f32 求值**，输出精度不受这一分层影响（理由见 §7.2）。

### 4.2 太阳 / 天光实时旋转

太阳方向是 Pass 1 的 uniform，旋转不触发任何重烘焙或资源重建：每帧重跑 Pass 1 即可。
Pass 1 **每帧处理全部 cluster**，不做「变化量低于阈值就复用上一帧」的时间抽取 ——
那会在旋转过程中造成逐簇跳变。流畅性靠**每帧代价恒定**保证，不靠跳过计算。

### 4.3 验收

| 项 | 门限 | 测量 |
|---|---|---|
| `EvaluatePRT64` 全链路 | **≤ 0.30 ms** @ 1080p，骁龙 7+ Gen 2 | `VK_EXT_host_query_reset` 时间戳，Pass 1–4 求和；口径见 §7.1 |
| 静态太阳 / 天光旋转 | 无可见卡顿（P99 帧时 ≤ 该场景中位数 × 1.2） | `PRISMA_BENCH_REPORT` 的 P50 / P99 |
| 1080p 全屏 | 渲染分辨率 == 输出分辨率，1:1，无上采样 | 分辨率断言 + 逐像素 golden image |

## 5. M3：动态光源与防漏光

### 5.1 Cluster-Major 传播

- **全量传播**：`64×32×64` 光照簇网格（与 GI 的 `64³` 对齐），每簇内光列表上限 64；
  超限时**报错并中止本帧的动态光传播**，不按光强静默截断 —— 砍光会让画面亮度跳变。
- 传播在**全部** cluster 上跑，不按「是否被点亮」做稀疏跳过：被跳过的簇保留上一帧值，
  会让移动光源留下拖尾。稀疏性只来自存储排布（cluster-major 连续段），不来自计算裁剪。
- 每个 cluster 的传播是一次 6 邻域加权平均 × 64 系数（`workgroup = 1 cluster`，64 线程各 1 系数）。

### 5.2 定点数 `InterlockedAdd`

移动端不保证 float32 原子（`shaderBufferFloat32Atomics` 在部分 Adreno / Mali 上缺失或慢），
因此**累加一律走 `uint32` 定点**：

- 值域：每通道 Q8.12（`[0, 16)`，分辨率 `2^-12`），打包为 `R|G|B` 三段；
- 累加：`InterlockedAdd(key, value)`，每个 cluster 三个 `uint32`；
- 读回：Pass 3 统一解码并做饱和，溢出不回绕（`min(sum, 0xFFFF)`）。

**这是唯一实现**，不提供 float 原子分支。

### 5.3 防漏光（20 cm 薄墙）

三道机制，缺一不可：

1. **叶子不跨墙**：64 树分裂判据（a）保证 cluster AABB 只落在墙的一侧，从源头消除「共用一个 cluster」。
2. **resolve 门控**：Pass 3 泼溅时，若体素中心到最近簇的连线落在墙的背面（用簇预存的
   `single_sided` 标志 + 该体素到簇中心的法向一致性判定），则拒绝该簇的贡献，权重归零而非衰减。
3. **权重门控**：体素采样时对三线性权重做 `max(w - leak_bias, 0)` 归一化，
   `leak_bias` 默认 `0.05`（可从 `settings.ini` 调整）。这是**抑制错误贡献**的
   修正项，不是降质量：它只削减本不该存在的跨墙权重，不削减合法权重。

### 5.4 验收

| 项 | 门限 | 测量 |
|---|---|---|
| 16 个动态点光，Pass 1–3 合计 | **≤ 0.15 ms** | GPU 时间戳分段；16 光全部开启、全部在视锥内 |
| 薄墙串光 | 墙暗侧像素的间接光亮度 ≤ 亮侧均值的 **2%**，且**无可见梯度** | golden image：20 cm 厚墙、两侧等距相机，逐像素比较 |
| 光源溢出 | 单簇光列表超 64 时显式报错中止，不静默截断 | 错误计数与日志断言 |
| 拖尾 | 移动点光源后的残留亮度 ≤ 稳态的 1% | 移动光源序列帧，逐像素比较 |

---

## 6. M4：流式加载与稳定性

- **区块粒度**：64 树 `level = 2` 的节点（`world_extent / 16`，500 m 场景下约 31 m）为流式单元，
  每单元独立 cluster 载荷段，节点表常驻。
- **队列**：`prism-asset` 的 `async-io` feature 承担 IO；主线程只做 `enqueue`，
  拷贝在传输队列上完成，主线程不做逐块阻塞等待。流式只决定区块「有没有」，
  不决定区块「有多准」——已加载区块的精度与分辨率不随流式预算变化。
- **预算与淘汰**：`MemoryBudget` 取 `min(显存预算, settings.ini 上限)`，LRU 淘汰非可见区块；
  淘汰在帧末归还到 pool，不在帧中 `vkFreeMemory`（避开 [lessons-learned.md](lessons-learned.md) 的 device-lost 陷阱）。
- **稳定性**：`PRISMA_BENCH_REPORT` 基线 + 30 分钟资源计数比对。

### 验收

| 项 | 门限 | 测量 |
|---|---|---|
| 切块主线程分配 | **< 0.2 ms**（GC / 分配合计） | 帧作用域分配计数器，记录 P99 |
| 连续 30 min | 无泄漏（`vk::DeviceMemory` 分配数与 RSS 单调不增） | 每 60 s 采样，末 10 min 斜率 ≤ 0 |
| 降频掉帧 | 帧时 P99 相对第 1 分钟漂移 ≤ 10% | 报告对比 |

---

## 7. 验收口径与测量方法

本节只定义**每条指标在什么口径下、用什么手段测量**。指标数值来自里程碑表，是目标，
不是本文件推导出的预测值。

### 7.1 口径定义

| 指标 | 口径 | 测量手段 |
|---|---|---|
| M1 端到端耗时 | `bake-prt` 进程 wall clock，排除首次 `scenes.pak` 构建、驱动初始化与 shader 编译 | 工具内计时报表 |
| M1 输出体积 | `.prtbin` 落盘字节总数（头部 + 节点表 + 载荷） | 文件大小；`leaf_count` 断言先行失败 |
| M2 `EvaluatePRT64` | Pass 1–4 GPU 时间戳之和，1080p，**全分辨率 1:1 采样** | `VK_EXT_host_query_reset` 分段计时 |
| M2 旋转流畅度 | 太阳 / 天光连续旋转下的帧时分布 | `PRISMA_BENCH_REPORT` 的 P50 / P99 |
| M3 16 动态光 | Pass 1–3 GPU 时间戳之和，16 光全部在视锥内且全部可见 | 同上，按 pass 分桶 |
| M3 薄墙串光 | 20 cm 墙两侧等距相机的间接光亮度比 | golden image 逐像素比较 |
| M4 切块开销 | 主线程帧作用域分配的 P99 | 分配计数器（非 GC 语言，统计 allocator 调用） |
| M4 长稳 | 30 min 连续运行的内存与帧时漂移 | 每 60 s 采样，比较首尾区间 |

**唯一测量口径**：所有指标都在全分辨率、1:1 采样、f32 运算下测量。
不存在「降质量口径」与「高质量口径」两套数字；指标不达标时改算法或改表示，不改采样率与精度。

### 7.2 为什么 64 系数求值放在 cluster 粒度

这是**结构性**取舍，不是降质量，也不依赖任何估算值：

- **读放大**：逐像素求值意味着每个像素读一份完整传输向量，读取量随像素数增长；
  cluster 粒度下读取量只随 cluster 数增长，与屏幕分辨率解耦。这是数据布局的分层 ——
  两种粒度下求解的数学量完全相同。
- **频域**：辐照度是低频量，高阶能量在 cluster → 体素的 resolve 积分中已经完成，
  像素层再引入 64 阶只是把同一份能量重复积分。
- **结论**：64 系数在 cluster 粒度求解、体素与像素层处理 2 阶，是表示层的正确分层；
  体素层与像素层都是全分辨率 f32 求值，输出不因分层而降精度。

### 7.3 质量红线（明确不做）

以下手段在本设计中**一律不采用**，即使能换取性能：

| 手段 | 为何不做 |
|---|---|
| 半分辨率 / 降分辨率 resolve | 间接光边缘糊化，且与 1:1 采样口径冲突 |
| 深度感知上采样、时间累积去噪 | 引入 TAA 类拖影与鬼影 |
| f16 用于运算与中间信号 | f16 只作为磁盘格式与传输向量的存储格式，读入即提升 f32 |
| 按变化量跳过 cluster（时间抽取） | 旋转太阳时出现逐簇跳变 |
| 按距离 / 可见性裁剪 cluster 或光源 | 漏掉视锥外的反弹贡献，属可感知画质损失 |

### 7.4 必须实测确认的量（不预设数值）

- 全分辨率下每 Pass 的实际开销（Pass 4 为主要项）；
- 500 m 城市场景的 BLAS 显存占用与可行的三角面上限 —— 对 M1 而言，
  **显存可能先于时间成为约束**，具体阈值由实测确定。

### 7.5 实现前需确认、但不阻塞格式与烘焙阶段的事项

1. **M2 预算**：全分辨率口径下若 0.30 ms 不可达，应先讨论算法或表示的调整，
   **不通过降低采样率或精度换取达标**。
2. **M1 场景上限**：确认 500 m 城市的三角面预算，用于决定 BLAS 是否分块构建。

---

## 8. PR 拆解（每个 PR 独立可验证，diff < ~500 行）

| # | 分支 | 交付 | 验证 |
|---|---|---|---|
| **A** | `feat/prt-format` | `prism-render::prt_volume`：`.prtbin` 编解码、f16 量化、`leaf_count` 断言、往返单测 | `cargo test -p prism-render`；体积断言 |
| **B** | `feat/prt-bake-cluster` | `bake-prt` 子命令：采样 + PCA / k-means 聚类 + 64 树构建 + 导出（复用 `bake_gi` 基础设施） | Sponza 跑通；报告 JSON 校验 |
| **C** | `feat/prt-bake-budget` | 体积 / 耗时预算门禁与 500 m 城市合成场景夹具 | `bake-prt` 在 CI 上的预算断言 |
| **D** | `feat/prt-eval-pass` | Slang `prt.slang`（`EvalPRT64Cluster`）+ Pass 1 / 3 / 4 接入 Render Graph + `gi_mode` 新取值 | 与既有 probe volume 的画面 A / B；GPU 时间戳 |
| **E** | `feat/prt-dynamic-lights` | Pass 2 全量传播 + Q8.12 定点 `InterlockedAdd` + Proxy OBB 剔除 | 16 光基准；拖尾与溢出断言 |
| **F** | `feat/prt-leak-fix` | 叶子单侧判据 + resolve 门控 + `leak_bias` 边界 clamp | 20 cm 薄墙 golden image |
| **G** | `feat/prt-streaming` | 64 树区块异步加载 / 卸载 + `MemoryBudget` + 帧预算队列 | 30 min 稳定性 + P99 分配 |
| **H** | `feat/prt-bench` | `PRISMA_BENCH_REPORT` 增加 `gi.*` 分段字段与口径标注 | 报告 schema 单测 |

依赖顺序：A → B → C → D → E → F → G，H 可并行。**A 的格式契约一旦合入，后续 PR 只能在其上扩展版本号，
不得就地改写字段语义**（1.0 前允许直接升版本，不写兼容读取）。

---

## 9. 与既有设计的边界

- **不新增 Render Graph 资源类别**：PRT 全部落在 `SceneScope`（节点表 / cluster 载荷）与
  `FrameScope`（`gi_volume` 双缓冲）内，见 DESIGN §6.1。
- **不复制 SH 求值**：64 系数基函数的 Rust 与 Slang 两侧实现必须由同一份表格生成
  （沿用 `gi.rs` 与 `gi.slang` 的 lock-step 纪律），并通过往返单测锁定。
- **不引入 float 原子**：见 §5.2。
- **不改 `ForwardPass` 合成公式**：PRT 与 probe volume 输出同一 `E(n)/π` 量纲，
  `scene_frag.slang` 的合成行保持不变（DESIGN §6.5）。
