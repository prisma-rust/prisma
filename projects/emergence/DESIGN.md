# Emergence（涌现）：v3.0 终极全景标签体系 — 围绕 Prisma 引擎的开发计划

> 项目目录：`projects/emergence/`（独立 workspace，与 `projects/template`、`projects/echowell`、`projects/fishing` 同级）。  
> 架构基准：严格遵循仓库 `AGENTS.md` / `GEMINI.md`、`docs/DESIGN.md`、`docs/REFLECT.md` 与 `docs/project-module-architecture.md` 的分层红线与 1.0 前零兼容原则。

---

## 1. 项目定位与引擎 / 项目分层硬边界

`Emergence` 是一个面向硬核涌现交互的物理与危化沙盒工程。本开发计划以 **Prisma 引擎底座能力演进** 为核心主线，将 v3.0 五大标签体系（基础环境状态枚举、固有材质位、瞬态危化位、空间拓扑位、外部刺激与动态时钟）拆解为可复用的通用引擎基础设施与可热替换的项目规则表。

### 1.1 硬边界契约（Hard Boundary Contract）

判定标准：**换一个化学/生态/工业模拟沙盒，该能力能否原样复用？** 能 → 进引擎 `crates/prism-*`；不能 → 留 `projects/emergence/`。

| 层次 | 所在 Crate | 允许承载的职责 | 严禁越界的职责 |
|---|---|---|---|
| **核心调度与并发层** | `prism-ecs` | 活跃集 / 休眠跳过（`DORMANT` 零开销迭代）、Chunk 并行作业（`jobs::scoped_parallel_chunks`）、多频固定步调度器 | 出现热传导、酸蚀、辐射、生物活性等具体物理公式 |
| **反射与场景协议层** | `prism-reflect` / `prism-scene` | `Bitflags` 字段元数据描述、紧凑二进制/RSCN v4 组件编解码、稳定 `TypeId`、RTTIBin 跨边界传输 | 写死 15 种材质或 18 种瞬态的具体位语义 |
| **空间几何与拓扑核** | `prism-physics2d` / `rapier3d`（`prism-app/physics`） | 确定性 AABB 碰撞（`Aabb2`、`raycast_aabb`）、网格邻接查询、连通域/结构支撑 Flood-Fill、复合刚体与动态体素岛 | 写入真空沸腾、高压液化或生物寄生蔓延逻辑 |
| **引擎运行时服务层** | `prism-engine` / `prism-app` | 资产根路径（`asset_paths`）、`DirtyRouter` 帧间脏数据路由、热重载、音频驱动与时间分频驱动 | 出现任何 ROM 物质配表或具体玩法状态机 |
| **渲染与视觉反馈层** | `prism-render` / Slang | `SpritePass` 2D 切片合批（`SpriteInstance::flags`）、**Compute DDA 三级体素光线步进**、光源与粒子、调试叠加层 | 按硬编码材质名写死渲染分支 |
| **编辑器与自动化层** | `prism-editor-api` / `prism-editor-mcp` | 位标志折叠检查器 UI Schema 投影、无头刺激注入与属性断言工具 | 链接 `prism-render` / Vulkan |
| **用户项目层** | `projects/emergence/` | `PhysicalCell` 16 字节内核、v3.0 5 维跃迁规则矩阵、ROM 物质配置表、实验室场景与交互探针 | 直接使用 `ash::vk`、绕过 `GameWorld` / `HostApi` 访问渲染线程或引入 egui 依赖 |

---

## 2. 底层紧凑内存模型与 3D 三级体素拓扑

为支撑海量网格与 3D 精细破坏在 60 Hz 下的零分配推演，v3.0 全景标签体系不采用分散的堆分配组件或字符串标签，而是统一压入 **16 字节 `#[repr(C)]` `PhysicalCell`**（实现在 [`projects/emergence/src/tags.rs`](src/tags.rs)）：单条 64 字节 CPU 缓存行可连续容纳 **4 个完整物理单元**。

在空间组织上，**16 字节 `PhysicalCell` 同时作为 2D 轻量实验切片与 3D 三级稀疏体素（L0 → L1 → L2）的统一叶子单元**：

### 2.0 3D 三级稀疏体素层级（64-Tree + 1 KB Brick）

| 层级 | 空间尺度 | 数据形态 | 核心职责 |
|---|---|---|---|
| **L0：Region（世界大块）** | `16×16×16` 个 L1 | 稀疏哈希表 / 流式槽 | 流式加载、全块 `DORMANT` 剔除、房间级气压 `Pressure` 与气密连通域 |
| **L1：64-tree 节点（`4×4×4` 子槽）** | 覆盖 `4×4×4` 个 L2 | 原生 `u64` 位掩码（`occupied` / `dormant` / `airtight` / `dirty`） | 单条 `popcnt`/`tzcnt` 跳过空格与静止区；承载管道 `CONDUIT`/阀门 `VALVE` 宏观拓扑、远景 LOD 与结构支撑锚点 |
| **L2：Micro Brick（`4×4×4` 叶子）** | `4×4×4 = 64` 格 | **`64 × 16 B = 1,024 B = 1 KB`** | `PhysicalCell` 连续阵列：对齐 16 条缓存行与 Vulkan Compute `local_size(4,4,4)`；承担酸蚀、脆裂、燃烧与 **Teardown 式毫米/分米级破坏** |

### 2.1 内存布局总览（Exact 16 Bytes）

```text
Offset   Size  Field              Packed Content
────────────────────────────────────────────────────────────────────────────
0x00     2 B   env: EnvPacked     Matter(2b) | Temp(3b) | Press(3b) | Density(3b) | Bio(2b) | FlowDir(3b)
0x02     2 B   material           15 Intrinsic Material & Physical Bits (u16)
0x04     4 B   transient          18 Dynamic Transient & Hazard Bits (u32)
0x08     2 B   topology           10 Spatial Topology & Affordance Bits (u16)
0x0A     1 B   pending_stimuli    8 Base Stimulus Input Channels (u8)
0x0B     1 B   durability         Structural integrity counter (0..=255)
0x0C     4 B   clocks             HALF_LIFE(15b) | SWITCH_HEAT(8b) | INTENSITY(4b) | TICK_HZ(5b)
────────────────────────────────────────────────────────────────────────────
Total = 16 bytes (const assert size_of::<PhysicalCell>() == 16)
```

### 2.2 五大维度比特级映射

1. **核心状态与环境枚举（`EnvPacked(u16)` — 精确占满 16 bits）**：
   - `MatterState`（bits `0..2`）：`SOLID (0)` / `LIQUID (1)` / `GAS (2)` / `PLASMA (3)`
   - `Temperature`（bits `2..5`）：`ULTRA_CRYO (0)` / `CRYO (1)` / `AMBIENT (2)` / `HOT (3)` / `PLASMA_HEAT (4)`
   - `Pressure`（bits `5..8`）：`HARD_VACUUM (0)` / `VACUUM (1)` / `NORMAL (2)` / `PRESSURIZED (3)` / `GIGA_PRESSURE (4)`
   - `Density`（bits `8..11`）：3-bit 离散阶梯 `0..=7`
   - `BioActivity`（bits `11..13`）：`INERT (0)` / `CRYPTOSIS (1)` / `VITAL (2)` / `HYPER_NEO (3)`
   - `FlowDir`（bits `13..16`）：`NONE (0)` / `N (1)` / `S (2)` / `E (3)` / `W (4)` / `IN (5)` / `OUT (6)` / `BI_DIR (7)`
2. **固有材质位（`MaterialFlags(u16)` — 15 bits）**：
   `METALLIC`, `ORGANIC`, `GLASS`, `CONDUCTIVE`, `INSULATED`, `FLAMMABLE`, `AIRTIGHT`, `PRESSURE_RATED`, `HOT_RESISTANT`, `RAD_SHIELDED`, `PIEZOELECTRIC`, `SUPERCONDUCTIVE`, `ELASTIC`, `ABSORBENT`, `OXIDIZER`。
3. **瞬态危化位（`TransientFlags(u32)` — 18 bits）**：
   `WET`, `BURNING`, `FROZEN`, `CHARGED`, `MAGNETIZED`, `BRITTLE`, `CORRODED`, `BROKEN`, `VOLATILE`, `TOXIC`, `CORROSIVE`, `RADIOACTIVE`, `INFECTED`, `STASIS`, `WARNING`, `DORMANT`, `SHARP`, `VISCOUS`。
4. **空间拓扑位（`TopologyFlags(u16)` — 10 bits）**：
   `RIGID_WELD`, `BIO_NEST`, `FROZEN_LOCKED`, `PUNCTURED`, `EDIBLE`, `EQUIPPABLE`, `SEALED`, `CONVEYOR`, `CONDUIT`, `VALVE`。
5. **外部刺激与动态时钟（`StimulusFlags(u8)` + `SignalClockState(u32)`）**：
   - 8 种基础刺激：`HEAT`, `COLD`, `SHOCK`, `ACID`, `BIO`, `KINETIC`, `RADIATION`, `LIGHT`。
   - 4 个寄存器：`HALF_LIFE`（0..=32767，15 bits）、`SWITCH_HEAT`（0..=255，8 bits）、`INTENSITY`（0..=15，4 bits）、`TICK_HZ`（0..=20，5 bits）。

---

## 3. 核心物理不变量与涌现跃迁机制设计

### 3.1 物态强制绑定密度阶梯与温压相图

- **宏观密度压制铁律**：`EnvPacked::new` / `set_matter` / `set_density_hint` 在写入瞬间强制钳位：
  - `GAS` / `PLASMA` 锁定在 `0..=1`（最轻档，同档内由材质微调如氢 vs 六氟化硫）；
  - `LIQUID` 锁定在 `2..=4`（中等档）；
  - `SOLID` 锁定在 `5..=7`（最重档）。
  从而在底层数据结构上杜绝“气态比固态重”导致的浮力反转与排序穿帮，自然驱动浮力分层、无泵热对流及 4~5 Hz 离心分离。
- **温压耦合相变**：
  - 真空沸腾：`LIQUID` 在 `VACUUM | HARD_VACUUM` 且温标 `>= AMBIENT` 时强制气化为 `GAS`；
  - 高压液化：`GAS` 在 `GIGA_PRESSURE` 且温标 `<= AMBIENT` 时强制压缩为 `LIQUID`；
  - 低温冻结：`COLD` 将温标推至 `CRYO | ULTRA_CRYO` 时，液体固化为 `SOLID` + `FROZEN`；非 `ELASTIC` 材质同步附加 `BRITTLE`（受 `KINETIC` 撞击直接碎裂为 `BROKEN`）。

### 3.2 生物活性四阶状态机（`BioActivity`）

- `INERT`：死物或非有机体基线，不触发代谢与感染蔓延；
- `CRYPTOSIS`：隐生休眠态——`ORGANIC` + `VITAL` 进入 `CRYO/ULTRA_CRYO` 或 `FROZEN` 后自动转入 `CRYPTOSIS`，同时冻结 `HALF_LIFE` 腐败时钟；回到 `AMBIENT` + `WET` 时复苏回 `VITAL`；
- `VITAL`：活体态——持续推进 `HALF_LIFE` 代谢时钟；若同时遭遇 `AMBIENT` + `WET` + `BIO` 刺激且无 `SEALED` 保护，触发 `INFECTED`（菌丝寄生），条件成熟转化为 `BIO_NEST` 产出树脂；
- `HYPER_NEO`：狂暴异变态——`VITAL` 受到未屏蔽 `RAD_SHIELDED` 的 `RADIATION` 刺激后发生畸变，无视常规代谢节流并对外释放 `BIO` / `TOXIC` 刺激。

### 3.3 危化预警与零开销休眠机制

- **`WARNING` 抢险窗口**：当高压容器即将超越 `PRESSURE_RATED` 上限、超导线圈即将失超（温标越过 `CRYO`）或反应堆进入临界前，强制留出 **3~5 秒**的 `WARNING` 状态，同步点亮 Shader 高频闪烁警报与音频通道警报声。
- **`STASIS` 静滞力场**：只要 `TransientFlags::STASIS` 置位，所有相变、化学反应、刺激消费及 `HALF_LIFE` 递减全部冻结。
- **`DORMANT` 休眠剪枝**：单元在无 `pending_stimuli`、无活跃 `HALF_LIFE` / `SWITCH_HEAT` 计时器、无 `BURNING` / `WARNING` / `INFECTED` 持续危害时自动置位 `DORMANT` 并移出活跃推演列表；仅当外部 `apply_stimulus` 注入信号或邻接网格写入脏位时即时唤醒。
- **`SWITCH_HEAT` 振荡熔断**：非 `SUPERCONDUCTIVE` 导体在低电平到高电平（`!CHARGED -> CHARGED`）跳变时递增 `switch_heat`，达到 `SWITCH_HEAT_MELTDOWN_THRESHOLD` 即刻烧毁为 `BROKEN` 并清空 `CHARGED`，从物理机制上封堵高频红石振荡导致的 CPU 死循环。

### 3.4 空间拓扑与输运管网

- **表面履带（`CONVEYOR`）**：沿 `FlowDir` 以 `TICK_HZ (0..=20)` 离散步进搬运固态实体；超频（接近 `20 Hz`）时在出口端赋予抛射速度矢量（投石机玩法）。
- **封闭管道（`CONDUIT` + `VALVE`）**：输运流体与气体，受连通域压差与同温密度差（无泵热对流）驱动；`VALVE` 受 `CHARGED` 状态控制通断；管道内液体触发 `FROZEN` 或陷入 `VISCOUS` 阻塞时导致上游压力跃升并触发 `WARNING` → `PUNCTURED` / 炸膛。
- **复合刚体、结构坍塌与容器拓扑（`RIGID_WELD` / `Teardown 式破坏` / `FROZEN_LOCKED` / `PUNCTURED` / `SEALED` / `EDIBLE` / `EQUIPPABLE`）**：
  - **结构连通性断裂（Teardown-Style Destruction）**：L2 体素被 `KINETIC` 爆破、`ACID` 蚀穿（`CORRODED`）、`COLD` 冷脆敲碎（`BRITTLE`）或高温熔断后，一旦某块结构与世界锚点（地基/承重墙）失去连通，即从静态 L0–L2 树剥离为**带有局部 L1/L2 子树的自由体素刚体岛（Dynamic Voxel Island）**；
  - 脱离后的刚体岛受 `RIGID_WELD` 约束整体旋转、下坠、撞击，并在接触瞬间按动量向世界网格反向注入 `KINETIC` 刺激（砸穿楼板、引爆 `VOLATILE` 储罐或压碎 `GLASS` 管道）；
  - 世界物体、背包存储格与生物器官网格共用同一套标签穿透与损伤规则。

---

## 4. 围绕 Prisma 引擎的子系统改造与扩展设计

以下各节逐一定义为了将 v3.0 沙盒机制完整落地，需要在 `crates/prism-*` 中增量演进的通用引擎能力（所有通用能力均不得包含特定玩法词汇）。

### 4.1 `prism-ecs`：休眠活跃集、多频分频与双缓冲并行 Chunk

现状：`prism-ecs`（[`crates/prism-ecs/src/lib.rs`](../../crates/prism-ecs/src/lib.rs)）已提供稀疏组件池 `World`、实体代次句柄 `Entity`、`World::set_active` 活跃过滤以及复用线程池 [`jobs::scoped_parallel_chunks`](../../crates/prism-ecs/src/jobs.rs)（上限 8 线程）。

引擎扩展需求：
1. **Chunk / Brick 级活跃位图与唤醒队列（Dirty Brick Registry）**：
   - 新增通用 `ChunkedGridStorage<T>` / `VoxelTreeStorage`（L1 `u64` 活跃位图 + L2 `4×4×4` 1 KB Brick）：当整个 L1/L2 处于 `DORMANT` 或全真空时，`popcnt == 0` 直接跳过，实现真正的 **0 CPU 占用**。
2. **四级确定性分频调度（Multi-Rate Fixed Scheduler）**：
   - 在固定步 `60 Hz` 基础上提供通用分频节拍令牌 `StepPhase`：
     - **60 Hz Base Step**：`KINETIC` 碰撞响应、动态体素岛刚体步进与输入采样；
     - **0~20 Hz Discrete Pulse Bus**：按实体/网格 `TICK_HZ` 相位累加器分发机械、传送带与电路脉冲；
     - **4~5 Hz Convection / Centrifuge Step**：跨格浮力交换、无泵热对流与离心分离（3D 流体宏观压差均化在 L1 节点执行，破口处下沉至 L2）；
     - **1 Hz Clock Decay Step**：`HALF_LIFE` 衰变/腐败递减、`SWITCH_HEAT` 自然散热与 `WARNING` 倒计时。
3. **多色棋盘格并行步进（2D 4 色 / 3D 8 色 Checkerboard）**：
   - 利用 `jobs::scoped_parallel_chunks` 将空间块按 `(cx % 2, cy % 2)`（2D 4 批）或 `(cx % 2, cy % 2, cz % 2)`（3D `2×2×2` 8 批）并发执行，确保跨 Brick 边界读写邻接单元时不产生数据竞争且完全确定可回放。

### 4.2 `prism-reflect` & `prism-scene`：位标志反射与 RTTIBin 零兼容穿越

现状：`ComponentRegistry`（[`crates/prism-scene/src/registry.rs`](../../crates/prism-scene/src/registry.rs)）通过 `TypeRegistry` 和 `#[derive(Reflect)]` 将组件投影给编辑器、RSCN v4 与 `preview-module` C ABI（[`crates/prism-game-api/src/lib.rs`](../../crates/prism-game-api/src/lib.rs)）。

引擎扩展需求：
1. **Bitflags 反射描述符扩展**：
   - 在 `prism-reflect` 中为带位标志语义的封装整型新增字段属性描述（`#[reflect(bitflags = "...")]` 或位名表映射），使编辑器检查器与 `prism-editor-api` 能直接渲染复选框组而不是裸十进制整数。
2. **Chunk / Cell 批量二进制组件与 RTTIBin 通道**：
   - 在 `prism-scene` 新增通用的 `CellGridChunk` 组件（固定尺寸字节切片 + 脏区版本 `u64`），确保 RSCN v4 场景打包与 `preview-module` 跨 C ABI 传输时仅同步变化 Chunk，避免逐格拆成上万个独立 `Entity`。
3. **静态物质 ROM 数据加载契约**：
   - 项目侧物质 ROM 表以纯 JSON 数据驱动放在 `assets/data/materials.json`，由 `GameWorld::asset_exists` / 热重载机制加载，不写死进引擎。

### 4.3 `prism-physics2d` / `rapier3d` & `prism-engine`：空间拓扑核、Teardown 断裂与动态体素岛

现状：`prism-physics2d`（[`crates/prism-physics2d/src/lib.rs`](../../crates/prism-physics2d/src/lib.rs)）已具备无 ECS 依赖的确定性 `Aabb2` 与 `raycast_aabb`；3D 刚体侧已有可选 feature `prism-app/physics`（`rapier3d`）。

引擎扩展需求：
1. **连通域与结构支撑拓扑求解器（Cavity & Structural Integrity Solver）**：
   - 提供增量 Flood-Fill 求解器：一方面输入 `AIRTIGHT` / `VALVE` 掩码划分气密舱室与管道网络 ID；另一方面在受破坏区域执行**局部 BFS/Flood-Fill 检测与世界锚点的连通性**，一旦失去支撑即返回脱离体素集合，触发 Teardown 式结构坍塌。
2. **动态体素刚体岛与复合碰撞桥接（Dynamic Voxel Islands + `RIGID_WELD`）**：
   - 脱离静态 L0–L2 树的体素块封装为带局部 L1/L2 子树的**动态体素刚体**，按体素质量分布计算质心与惯性张量，由复合 AABB/Convex 代理接入 `rapier3d`（或 2D 切片中的 `prism-physics2d`）参与自由旋转、滚落与碰撞；
   - 碰撞解算返回法向冲量 `impulse: f32` 与接触点坐标，供上层按阈值分发 `KINETIC` 刺激（驱动自身与目标网格的 `BRITTLE` 碎裂、`PIEZOELECTRIC` 放电、`SHARP` 穿刺或 `VOLATILE` 殉爆，实现砸穿楼板与二次解体）。
3. **帧间脏追踪集成**：
   - 接入 `prism-engine::dirty_router::DirtyRouter`（[`crates/prism-engine/src/dirty_router.rs`](../../crates/prism-engine/src/dirty_router.rs)），当某网格区域或体素岛视觉/光照状态未发生变更时跳过渲染实例与 GPU Brick 缓冲重传。

### 4.4 `prism-render` & Slang 着色器：瞬态视觉编码与 3D Compute DDA 体素渲染

现状：`SpritePass`（[`crates/prism-render/src/sprite_pass.rs`](../../crates/prism-render/src/sprite_pass.rs)）将所有 2D 实例合并为单次 draw call（`SpriteInstance::flags` 当前使用 `bit 0`）；同时仓库已有 64 叉树与 Compute 管线演进基础（见 [`docs/prt-gi-64tree-design.md`](../../docs/prt-gi-64tree-design.md)）。

引擎扩展需求：
1. **2D 切片快速路径（`SpriteInstance::flags` 多状态着色器通道）**：
   - 保留 `bit 0` 为 `receive_lights`，将高比特位开放为通用视觉效果调制位，在 `assets/shaders/slang/sprites.slang` 中以零分支位运算渲染瞬态表现：
     - 表面高光/湿润压暗（`WET`）、结晶冰霜边框（`FROZEN` / `FROZEN_LOCKED`）、自发光与警报频闪（`BURNING` / `CHARGED` / `WARNING`）、酸蚀/菌丝叠加（`CORRODED` / `INFECTED`）、静滞定格轮廓（`STASIS`）。
   - 若修改 `sprites.slang`，同步运行 `bash assets/shaders/compile.sh` 与 `xtask shader_bindgen`（严格对齐 `shader_bindings::sprites`）。
2. **3D 三级体素 GPU Compute DDA / Raymarching 通道（免三角网格重建）**：
   - 新增 `VoxelDdaPass` Compute 着色器：直接步进 L0 Region → L1 `u64` 位掩码（`firstbithigh` 硬件跳过空格）→ L2 `4×4×4` 1 KB Brick 缓冲，**彻底避免 Teardown 式高频破坏下反复重建 Greedy Mesh 的 CPU/GPU 卡顿**；
   - 支持动态体素刚体岛（`RIGID_WELD`）的局部逆变换光线求交，原生处理 `GLASS` 折射、体素缺口实时阴影、火光/电弧自发光与烟尘体积光照。
3. **光照、粒子与热力调试叠加层**：
   - 复用 `Light2D` / 3D 光源、`ParticleEmitter2D`（飞溅碎片、沸腾蒸汽、酸雾、电弧火花）与 `LineRenderer`（激光/电弧/`FlowDir` 矢量）；
   - 扩展引擎调试图热键：温标热力图、气压等压线、`DORMANT` 休眠块线框、结构支撑连通域着色与 `INTENSITY (0..15)` 梯度。

### 4.5 `prism-audio`：危化声光报警与脉冲节流

现状：`prism-app` 通过 `Subsystem::Audio` 与 `GameWorld::play_sound` 驱动 2D 空间化音效与混音通道。

引擎扩展需求：
1. **高频脉冲音效节流阀（Audio Voice Coalescing）**：
   - 当 `20 Hz` 传送带或上百个 `PIEZOELECTRIC` 单元同时触发 `KINETIC` 时，同一音色在单帧或 50 ms 窗口内按空间距离合并为最大并发数上限（如每类 ≤ 4 路），防止混音爆音。
2. **`WARNING` 临界预警总控**：
   - 维护持续循环的预警声道引用计数：任意连通域进入 `WARNING`（3~5 秒抢险窗口）即拉起空间定位报警声，故障解除或进入 `BROKEN` 后立即切换为泄压/爆裂音效。

### 4.6 `prism-editor-api` & `prism-editor-mcp`：位掩码检查器与因果链自动化回归

现状：`prism-editor-mcp` 暴露了文档读写、组件增删、游戏运行与截图工具。

引擎扩展需求：
1. **Inspector 位标志分组渲染**：
   - `prism-editor-api` 识别 `Reflect` 的位标志元数据后，在 egui Inspector 中按「材质 / 瞬态 / 拓扑」三组可折叠标签勾选。
2. **MCP 无头物理因果链探针（Headless Causal Assertion）**：
   - 为验证复杂涌现链条，后续在工程或 MCP 测试接口暴露确定性场景加载 + 指定坐标注入 `StimulusFlags` + N 帧步进后读取 `PhysicalCell` 断言，配合 `PRISMA_SCREENSHOT` 验证渲染表现。

---

## 5. 引擎需求映射与验收标准总表

| 引擎模块 | 由哪类 v3.0 机制驱动 | 核心改造内容 | 可量化验收标准 |
|---|---|---|---|
| `prism-ecs` | `DORMANT`、`TICK_HZ (0..20)`、`HALF_LIFE`、离心 `4~5 Hz` | 分块/Brick 休眠位图、四级固定分频、4 色/8 色棋盘格并行 | 10,000 静止体素 CPU 耗时 < 0.05 ms；并行结果与单线程逐位一致 |
| `prism-reflect` / `prism-scene` | 15 材质 / 18 瞬态 / 10 拓扑位、C ABI 预览模式 | Bitflags 描述投影、`CellGridChunk` / `VoxelBrick` RTTIBin 编码 | `preview-module` 动态加载热重载无 schema mismatch，位面板可勾选编辑 |
| `prism-physics2d` / `rapier3d` | `AIRTIGHT`、`CONDUIT`、`RIGID_WELD`、Teardown 坍塌 | 气密连通域与结构支撑 Flood-Fill、动态体素刚体岛、冲量回调 | 承重柱切断 1 帧内剥离悬空岛并自由砸落；破孔 `PUNCTURED` 即时失压 |
| `prism-render` / Slang | 3D 破坏渲染、`WET`/`BURNING`/`FROZEN`/`WARNING`/`STASIS` | 2D `SpriteInstance::flags` + 3D `VoxelDdaPass` Compute 步进 | 高频破坏零三角网格重建卡顿；单帧可见体素缺口与瞬态着色；`xtask shader-budget` 绿灯 |
| `prism-audio` | `WARNING`（3~5 s 窗口）、高频 `KINETIC`/`SHOCK` | 空间音频并发节流与预警声道生命周期 | 同屏 100 次撞击无爆音，临界前 3 秒稳定触发声光预警 |
| `prism-editor-mcp` | 全套因果链回归与截屏门禁 | 刺激注入指令 + `PhysicalCell` 状态读回 | 自动化脚本可独立复现「冷脆→撞击碎裂→漏液导电→开关熔断」全链路 |

---

## 6. 里程碑规划（M0–M5）与风险控制

### 6.1 里程碑切分

| 里程碑 | 交付内容 | 硬性出口门禁（Exit Gate） |
|---|---|---|
| **M0：骨架与内存内核**（当前已完成脚手架） | 建立 `projects/emergence/` 独立 workspace；落地 16 字节 `PhysicalCell` 与 5 维枚举/位标志全集及单元测试；完成引擎驱动规划文档 | `cargo check` / `cargo test`（`preview-module`）与 `scripts/check_comments.py` 全绿；`size_of::<PhysicalCell>() == 16` 编译期断言通过 |
| **M1：分块休眠核与基础物态相变** | `prism-ecs` 接入 Chunk 休眠位图与棋盘格并行；实现温压相图（真空沸腾、高压液化、密度强制阶梯）及基础 ROM 物质表 | 单元测试覆盖「密闭容器升温→`WARNING`→破孔失压→水真空沸腾」；全静默网格实现 `DORMANT` 零开销 |
| **M2：电/磁/热/腐蚀/压电闭环** | 实现 `INTENSITY (0..15)` 阻抗衰减、`SWITCH_HEAT` 过载熔断、`WET` 短路导电、`PIEZOELECTRIC` 敲击发电、`GLASS` 抗酸脆裂 | 构建可交互的“湿电熔断实验室”垂直切片：水漫绝缘地板引发导线短路升温熔断并触发音频与火花粒子 |
| **M3：流体管网、气密拓扑与 3D 三级体素** | 上线 L0/L1(`u64`)/L2(`1 KB Brick`) 三级存储与 Compute DDA 渲染；打通 `CONDUIT` + `VALVE` 压差与热对流、`FROZEN` 堵管炸膛、`CONVEYOR` 超频投石机 | 能够搭建闭环热交换管路与 20 Hz 投石发射台；3D 场景实时挖掘/炸孔零三角网格重建卡顿 |
| **M4：Teardown 式结构坍塌与生物核素演化** | 接入承重连通性 Flood-Fill 与 `RIGID_WELD` 动态体素刚体岛（砸穿楼板二次引爆）；实现 `BioActivity` 四阶、`BIO_NEST`、`HALF_LIFE` 与 `STASIS` | 炸断承重柱触发整片建筑倾倒砸碎下层储罐；完成「孢子复苏→寄生→辐射异变→静滞锁定」全链路 |
| **M5：着色彩蛋、调试视图与自动化门禁** | 完善 `sprites.slang` 与 `VoxelDdaPass` 瞬态着色、多通道热力/支撑调试图、接入 `PRISMA_SCREENSHOT` 与 MCP 因果链测试套件 | 支持桌面独立二进制 + 编辑器 `preview-module` 双形态 60 FPS 稳态运行；CI `user-projects` 构建全绿 |

### 6.2 关键技术风险与规避对策

| 风险点 | 触发诱因 | 规避策略 |
|---|---|---|
| **高频振荡导致卡顿或锁死** | 红石门电路 / 阀门在单秒内无限次翻转触发全图重算 | 硬件级嵌入 `SWITCH_HEAT` 熔断器与 `TICK_HZ <= 20` 上限；触发阈值直接写入 `BROKEN` 物理断路 |
| **气密与承重 Flood-Fill 帧率抖动** | 每次状态微调或破坏都全图扫一遍 BFS | 仅当 `AIRTIGHT`/`PUNCTURED`/`VALVE` 或支撑体素发生销毁时才在局部包围盒增量重算连通性；其余帧 O(1) 查表 |
| **动态体素刚体岛数量爆炸** | 大规模连环爆破瞬间剥离上百个碎块刚体 | 小于阈值体积（如 `< 8` 格）的孤立碎块直接转为粒子/瞬态碎片 (`SHARP`) 销毁，只有大块结构才升级为 `rapier3d` 动态体素岛 |
| **编辑器 `preview-module` C ABI 数据撕裂** | 将上万格拆为独立 ECS `Entity` 导致每帧 RTTIBin 序列化超时 | 严格以 `CellGridChunk`（连续块字节流）作为跨边界同步粒度，结合脏版本号增量传输 |
| **渲染通道与业务耦合污染引擎** | 在 `prism-render` 中写死“毒气绿色”“酸液冒泡”等玩法分支 | 着色器只读取泛化的视觉物理位（高光湿度、晶化边缘、脉冲自发光、抖动频闪），具体配色由项目侧 ROM 调色板传入 |
| **多平台编译锁与注释门禁失败** | 忽视 `Cargo.lock` 同步或 Rust 源码中出现中文注释 | 所有项目代码与引擎扩展严格使用英文注释，提交前强制走 `check_comments.py` + `cargo clippy -- -D warnings` |
