# Prisma 路径追踪修复与优化实施规格

日期：2026-11-13。状态：提议，尚未实施或完成性能验收。
作者：JasonGu 的 AI agent（GPT-6.1 Sol / OpenCode）。

## 1. 目标、范围与完成定义

以 `projects/sponza` 为主要回归场景，修复路径积分、材质命中和 SHaRC 契约，建立可信的性能基线，再优化射线、纹理和调度成本。结果必须能复用于引擎、独立 Player 和编辑器 Game View。

交付分为三个层次：

1. 必须完成：可重复基准、材质分类、可见性查询正确性、SHaRC 辐射与权重修复、独立训练、历史失效规则。
2. 必须评估并记录取舍：单灯快速路径、纹理 LOD、AS 生命周期、线程组、缓存与输出带宽。
3. 后续扩展：时空重建、降噪、稀疏采样、SER、OMM、RT pipeline、异步队列。不得把它们作为基础修复的依赖。

基础完成不以某个 FPS 数字定义。必须同时提供正确性证据、同条件 GPU/CPU 时间、显存及缓存统计；无测量不得声称某项是主要瓶颈或保证若干倍提升。60 FPS / 16.67 ms 只有绑定具体 GPU、分辨率、质量档后才是有效目标。

不在本次基础范围：重写 ECS、迁移到 D3D12、引入编辑器依赖、NRC SDK 接入、默认开启移动端全路径追踪。

## 2. 参考版本与证据边界

| 本地仓库 | 调查时 HEAD | 用途 |
|---|---|---|
| Prisma | `d1423c2d0072a1618f946c71c6df7427d2625183` | 当前实现 |
| RTXGI | `04372d7f91bb3a0444f9fd95d3f34381ea4e3b39` | 简洁的 SHaRC update / resolve / query 参考 |
| RTXPT | `303b408ab81790a5051abd7573830b7a4112eb8e` | v1.9.0 管线、ray cones、材质可见性、训练与重建 |

调查读取的是工作区文件；HEAD 不代表包含全部本地改动。RTXGI 存在本地修改和子模块状态变化；制作正式比较报告时须记录三个仓库的 dirty 状态及相关补丁摘要。本文没有运行三方同条件基准，也没有 Nsight 捕获。

下面位置均为相应仓库相对路径，行号仅用于此快照定位：

| 观察 | 本地证据 | 结论强度 |
|---|---|---|
| Prisma BLAS 未设置 OPAQUE，TLAS 未强制 opaque | `crates/prism-render/src/acceleration_structure.rs:444`、`bake_common.rs:290` | 已确认；耗时占比待测 |
| 所有非 opaque 候选执行 UV / alpha 测试，阈值写死 0.5 | `assets/shaders/slang/pt_render.slang:640` | 已确认 |
| 当前 GPU 材质和上传输入无 alpha mode / cutoff 字段 | `crates/prism-render/src/managers/material_manager.rs:58,111` | 已确认；不能从贴图 alpha 推断材质域 |
| 间接命中先累加 emission / NEE，再查询缓存 | `pt_render.slang:619–721` | 已确认；重复计能需按本地缓存定义验证 |
| UpdateHit 返回值被忽略，PT 中无 SetThroughput 调用 | `pt_render.slang:721`、`sharc_common.slang` 的对应接口 | 已确认的契约缺口 |
| 查询与稀疏更新同 shader，更新路径也可查询后退出 | `pt_render.slang:279–292,707–721` | 已确认；寄存器代价待测 |
| ReSTIR 固定 8 个候选，尝试时间与 4 邻居复用 | `pt_render.slang:34–35,309–349` | 已确认；候选评估不是同等数量阴影射线 |
| 材质贴图固定 SampleLevel 0 | `assets/shaders/slang/path_integrator.slang:315–388` | 已确认；采样缓存与带宽影响待测 |
| 静态场景上传将顶点烘到世界空间，按缺失几何触发 | `crates/prism-render/src/graph_renderer.rs:2700–2759` | 不应误报为每帧重建，动态更新需审计 |
| Sponza 默认开发命令可能运行 debug | `scripts/dev.sh:25,72–74`、`projects/sponza/Cargo.toml` | 配置事实；不证明用户当时运行的构建 |

本地 glTF 统计为 262267 个三角形、25 个材质（22 OPAQUE、3 MASK）；这只是源模型统计，正式报告必须同时记录运行时实际加载的包、子网格、实例和材质数。

## 3. 从 RTXPT 采用什么

| 参考 | 定位 | 本引擎决策 |
|---|---|---|
| 统一路径积分与参考模式共享输运实现 | `Docs/Design.md:30–40` | 保留共享 BSDF / 材质 / 采样逻辑，建立无缓存参考模式 |
| 独立低分辨率训练，再 resolve | `Rtxpt/Sharc/SharcIntegration.cpp:397–510` | 独立 compute 训练与消费变体；不需要先接 RT pipeline |
| 缓存查询返回从该顶点开始的辐射，调用方避免重复 emission | `Rtxpt/Shaders/PathTracer/PathTracerSHaRC.hlsli:255–298` | 先定义本地契约，再调整积分顺序，不机械复制字段 |
| 更新处理返回值与每段 throughput | 同文件 `359–395`；RTXGI `Pathtracer.hlsl:757,903–906` | 统一量纲、权重与终止规则 |
| 可见性 any-hit first-hit；alpha 只读必要数据 | `PathTracerBridgeDonut.hlsli:117–142`、`DonutBridgeCommon.hlsli:85–122` | 精简 alpha 命中路径，按真实材质域设置 flags |
| ray-cone 纹理采样 | `PathTracer/Utils/TextureSampler.hlsli:90–142`、`TexLODHelpers.hlsli` | 分阶段加入着色纹理 LOD；alpha 独立验证 |
| 静态 BLAS 重用、动态几何 refit、重建输入 | `Docs/Design.md:48–50,130–136` | 采用生命周期原则；不复制“每帧 TLAS 重建”作为默认 |
| 主 PT 可统一，训练、重建仍分阶段 | `Docs/Design.md:104–136` | 拆训练不等于改 wavefront；是否进一步拆 PT 由 profiler 决定 |

RTXPT 的实时主配置包含 DLSS-RR、可选低内部分辨率和稀疏像素追踪；README 的性能数字不是本引擎收益预测。其 Vulkan SER / OMM 仍列为进行中（`README.md:68–75`），D3D12 异步 DLSS overlap 也不能直接套到本 Vulkan 后端。

参考算法与复制源码、接入 SDK 分开审批。RTXPT 根许可证是 NVIDIA RTX SDKs LICENSE；若移植源码或 SDK，先记录具体文件及子模块许可证、署名和分发义务，不假定整仓为 MIT。

## 4. 必须保持的架构契约

- `prism-render` 持有 Vulkan 资源与 pass；`prism-engine` 映射作者设置与派生状态；项目只提供场景和游戏逻辑。
- 保持 Player UI-framework-free、editor-free；不把 cooker / SDK 工具链拉入玩家依赖树。
- 沿用右手系、相机 −Z、列主序、Vulkan Y flip、深度 [0,1]。运动向量必须单独写明单位、方向、jitter 约定。
- 新 pass 实现 `RenderPassNode`，图中声明资源读写与生命周期。不能新增图外“隐形”训练或 resolve。
- push constants 通过 Slang reflection 与 `shader-bindgen` 生成；GPU POD 的大小、offset、对齐必须可验证。
- 不支持 ray query 或 SHaRC 原子能力的设备保持现有光栅 / baked GI 回退。能力不足时记录有效模式，不能显示“已开启”却静默不执行。
- `docs/DESIGN.md:88` 的“实时 SHARC 已移除”与 §12.2 当前 feature 表矛盾；实施前同步权威文档，明确移动端 baked GI 与可选桌面 PT+SHaRC 的边界，修订旧蓝图的“更新/查询同管线”建议。

## 5. 基准与观测规格（第一阶段）

### 5.1 两种比较必须分开

算法成本比较：Release、固定相机/灯光、同一实际几何与贴图、同一内部尺寸（首选 1920×1080）、1 spp、对齐 bounce 语义、关闭超分/帧生成/稀疏采样/降噪；分别测 cache off/on。太阳角半径、环境、alpha、背面、transmission、RR 和曝光也必须记录。无法对齐的项明确列差异，不能称严格等价。

产品效果比较：允许重建与超分，记录实际渲染像素、spp、GPU ms、输出尺寸、视觉质量和重建延迟。帧生成后的显示 FPS 不用来证明路径追踪成本下降。

Sponza Release 启动基线：

```bash
cargo run --release --manifest-path projects/sponza/Cargo.toml
```

运行前按项目 assets 清单构建场景包，确认实际加载的包路径与摘要。`settings.ini` 的 build profile 不能代替 `--release`。同一时间只运行一个基准应用。

### 5.2 采样与报告

- 固定 seed 和帧序列；记录 GPU、驱动、API、时钟/电源状态、适配器、二进制与 shader 摘要、编译优化、validation、输出/内部尺寸、有效设置。
- 预热默认 120 帧、采样 600 帧、至少 3 次独立运行；另测冷缓存与缓存趋稳曲线，不能只测无限积累后的静止画面。若 120 帧尚未稳定，按预先定义的条件延长并记录。
- 复用 `PRISMA_BENCH_REPORT` 的分布与 `GpuTimer`，新增数据应版本化或写旁车报告；CPU 帧时间不可标成 GPU 时间，不支持时间戳时标记缺失。
- GPU 区间至少覆盖 AS、训练、resolve、主 PT、历史/重建、后处理以及整帧；分段和不等于整帧关键路径。所有图外工作也必须被覆盖，计时容量不足不得静默漏项。
- 统计 primary / scatter / shadow rays、nonopaque candidate、alpha fetch、平均路径长度、按 bounce 的 cache eligible/query/hit、训练路径与写入成功率、hash 占用/碰撞/插入失败、训练/resolve 成本。
- 统计使用抽样或诊断变体，正式性能采样关闭高频全局原子；记录仪表开销，空统计与不支持分开。
- 报告保留 p50/p95/p99、帧分布、显存峰值/常驻、冷启动 AS/上传时间、截图及原始样本。重复运行波动超过 3% 时先解释噪声。

### 5.3 场景矩阵

Sponza 至少包括日照中庭、阴影走廊、近景镂空、远景高频纹理、快速转向、相机切换、太阳移动、灯光开关、资产重载。另配微场景：opaque 带透明纹理、不同 cutoff 的 MASK、双面叶片、背面与镜像变换、BLEND 与 transmission、纯镜面、发光面、无灯/单灯/多灯、重复实例与动态对象。

## 6. 材质域、alpha 与 AS 修复

### 6.1 数据链先行

审计 glTF 导入 → 中间数据 → cooker/RMAT → runtime material → engine upload → GPU material → PT geometry。显式保存 OPAQUE/MASK/BLEND、alpha cutoff、double-sided；不要用“贴图存在 alpha”或 alpha 值是否为 1 推断域。transmission 不等于 alpha blend。

现有 `GpuMaterial` / `MaterialUploadInput` 没有完整字段，必须先写数据格式与 API 设计。RMAT 如需升级，定义版本、旧包默认值、拒绝/兼容策略及重烘焙路径；禁止利用未说明的 reserved 字段静默改变格式。旧材质不能盲目全部设 opaque；先调查既有语义，必要时保留显式兼容路径。

### 6.2 遍历规则

- OPAQUE geometry 设置 `vk::GeometryFlagsKHR::OPAQUE`，不执行 alpha 纹理测试；MASK 保持 nonopaque，使用对应 cutoff 与 base color alpha 因子。
- 按 geometry/submesh 分类，混合材质实例不能使用全实例 FORCE_OPAQUE；BLAS 共享键必须包含影响构建的材质分类。
- BLEND 选择并文档化透射/随机透明/暂不支持的有效行为；不能当 MASK 或简单 opaque。
- 明确单面/双面与反向缩放的 flags；不能为了性能统一打开背面剔除。
- 阴影只需要二元遮挡时使用 ACCEPT_FIRST_HIT_AND_END_SEARCH；保留完整 Proceed / commit 流程，MASK 孔洞继续遍历。阴影距离正确排除光源自身，透射阴影按另行定义的语义处理。
- alpha helper 只读取索引、UV、alpha 材质参数与必要纹理；不要求法线/切线/完整表面。是否被编译器消除须检查实际 shader。

验收：纯 opaque 微场景 alpha candidate/fetch 为零；混合场景 MASK 孔洞、cutoff、双面结果正确；opaque 的贴图 alpha 不改变遮挡；CPU 分类测试、GPU 画面对照和 validation 全通过。不能靠把所有材质强制不透明来达标。

## 7. 路径积分与 SHaRC 正确性

### 7.1 统一辐射契约

先为共享积分器定义：局部辐射 `L(x)`、相机累计吞吐 `beta`、单次散射权重 `w=f*cos/pdf`、emission、NEE、环境 miss 和 cache value 的量纲。沿用现有 BSDF 是否已包含 cosine 必须逐函数核实，避免重复乘。

本规格选择缓存代表“从该顶点出发的剩余出射辐射”，包含哪些 emission/直接项必须在实现与测试中唯一确定。初始实现不做材质 demodulation；如以后启用须同时定义编码/查询与缓存清空规则。

消费路径：构造命中信息与有效性 → 查询缓存 → 命中则只累加 `beta * L_cache` 并终止 → 未命中才累加约定的 emission、NEE、采样 BSDF 并继续。若本地 SHaRC 只能提供不含某项的缓存，先修订契约与对应位置，不混用 RTXGI / RTXPT 版本假设。

训练路径：计算局部 lighting → `SharcUpdateHit` 并处理 false → 以单段散射权重推进 SHaRC throughput → 继续。禁止将相机累计 beta 再乘进局部样本或重复推进历史权重。miss 必须传播环境，返回 false 后不再发无效散射射线。

共享检查包括 RR 的存活概率与权重补偿、bounce 上限含义、emissive hit 与 NEE 的 MIS、delta 灯采样的离散概率、transmission 的分支概率/折射权重、几何/着色法线、自交偏移。前述项目属于审计目标，未逐项证明存在 bug。

### 7.2 测试与参考

建立共享输运的无缓存参考变体，关闭 ReSTIR、裁剪和降噪中的有偏项，输出线性 HDR。先验证解析微场景，再用它检查缓存，不让两个同源错误互相认证。

必测：恒定环境/白炉能量、两段 throughput（如逐段 0.5）的传播、纯 emission、单方向光、cache hit 不再累计本地 NEE/emission、UpdateHit false 的终止、miss 传播、delta 路径不被粗糙缓存替代、NaN/Inf 与空 hash。

SHaRC 有偏，不能要求与无缓存每像素完全相同。初始质量门槛：约定漫反射 ROI 相对平均亮度误差 ≤5%，无 NaN/Inf、无稳定双倍计能；复杂材质另设阈值。参考采样量必须收敛到低于验收容差的统计不确定度。

## 8. 独立 SHaRC 训练、resolve 与消费

建议图顺序：

```text
Scene/AS ready → optional cache clear → SharcTrain → barrier
              → SharcResolve → barrier → PathTraceConsume → history/post
```

沿用 Vulkan RayQuery compute；共享命中、BSDF、灯光代码，编译为 no-cache / train / consume 三种受控变体。消费变体不携带训练传播数组，训练不使用屏幕 ReSTIR 历史。训练可按 SHaRC 自身的 resampling 规则停止，不能直接继承消费路径的缓存终止。

训练初始 downscale 沿用 5，另评估 4/8；使用 `ceil(width/d)`、边界检查和每块随帧变化的样本位置，避免固定模采样长期遗漏几何。设置训练 bounce 与容量预算；每种采样方案记录实际路径数量。

显式声明 transfer clear→compute、train 原子写→resolve 读写、resolve 写→consume 读以及跨帧 cache 访问依赖。多帧并行不能在下一帧训练时覆写仍被消费的缓存；先用同队列明确序列，双缓冲/时间线方案须写内存与延迟代价。

生命周期：resize 更新训练尺寸与屏幕历史；相机移动清屏幕累计/重投影历史但通常保留世界缓存；场景卸载、几何/材质语义变化、cache 格式变化清空或局部失效。灯光改变定义硬清空或有界刷新，报告追光延迟；改变曝光不应直接污染线性辐射缓存。冻结训练、resolve 与查询开关的组合行为必须明确。

验收：三阶段均有 GPU 区间；consumer 无训练写；非整除分辨率、热重载、相机切换、场景切换无越界/旧句柄；同预算比较冷/暖缓存质量、命中率及总成本。不能仅提高命中率牺牲画面来宣称成功。

## 9. 灯光采样与 ReSTIR

零灯/单灯走简单 NEE；仅在有效采样域确实只有一个解析灯且不存在需采样的 emissive/environment 时跳过 reservoir。多灯保留可切换基线，评估候选数 1/2/4/8 的成本与等时间误差。

当前时间复用使用上帧 reservoir，并不能据此认为已有可靠重投影；必须审计相机/物体运动、深度/法线/材质拒绝、光源 ID 稳定性、失效和权重归一。未完成有效历史拒绝前，移动场景默认使用可靠的非时间复用路径。

emissive 与环境采样纳入统一域/PDF 设计，处理 NEE 与 BSDF 命中 MIS；不要只为 Sponza 的太阳写死旁路。RTXPT NEE-AT / RTXDI 仅为后续多灯方案候选，基础阶段不引入新 SDK。

验收：单灯路径 reservoir 读写为零且结果无偏差；多灯静止与运动均与参考匹配统计容差；更少候选必须以等时间图像误差比较，不能只报更快。

## 10. 纹理 LOD、数据访问与 AS 生命周期

### 10.1 着色纹理

用 camera footprint 初始化 ray cone，随距离和散射传播，结合三角形 UV/世界空间面积与纹理尺寸求有限、受 mip 范围限制的 LOD。覆盖非均匀缩放、退化 UV、无 mip、normal map；验证上传与 `.pak` 确实含 mip 链。RTXPT shading 使用 ray cones，但其 alpha 测试仍采 mip 0，不能声称它解决了所有 alpha LOD。

第一步保持 alpha visibility 既有 mip 策略，仅优化着色纹理；之后单独评估保覆盖率 alpha mip 或保守方法。不能直接将粗 mip 用于 MASK 并接受叶片消失。

测量 texture cache/bandwidth、LOD 分布、远景闪烁和近景细节；“全改为固定更粗 mip”不算完成。

### 10.2 几何与构建

先用计数核实当前静态场景没有重复上传/BLAS/TLAS 构建。逐步从每实例世界空间复制改为共享 object-space BLAS + TLAS transform；共享键包括 mesh/submesh、拓扑与 opacity 分类。变换只更新 TLAS，拓扑变化重建 BLAS，形变才评估 refit 与退化后的重建。

拆分 position/UV-only 的遍历数据是否值得由显存、加载与 AS build 实测决定，不盲目重写通用 Vertex。静态 compaction 异步完成，旧 BLAS/TLAS 必须等待 GPU 使用结束才销毁；记录 scratch、峰值和常驻字节，禁止稳态 device_wait_idle。

验收：静态 600 帧计数无无故重建；重复实例增加时几何存储不线性重复；动态/镜像缩放/材质域切换/热重载正确；初始化与稳态成本分别报告。

## 11. Shader、带宽与调度优化

正确性稳定后比较 8×8、16×8、16×16 等线程组，记录实际寄存器、spill、occupancy、warp/wave 活跃比例和 trace/memory stalls。禁止仅凭 shader 长度判断需要 wavefront。先让消费路径去除训练状态，再考虑拆可见性/着色或材料变体，并限制 permutation 数量。

累积保持 FP32 参考；显示中间输出可评估 FP16，reservoir 可评估紧凑布局；每次记录实际资源大小与误差。以 1080p 为例单张 RGBA32F 约 31.6 MiB，仅容量不等于实际带宽。禁止用压精度遮掩积分错误。

只保留性能证据超过噪声、质量不退化的候选；无收益的线程组、压缩或 pass 拆分记录后放弃。审计 CPU prepare / extract / 提交和重复分配，使用现有 packet 重用；GPU 优化不自动解决 debug CPU 瓶颈。

## 12. 后续重建与硬件扩展

重建先定义通用输入：线性 HDR、深度、几何/着色法线、roughness、albedo、可靠 motion vectors、前帧 transform、jitter、曝光及 reset。背景、无效运动、遮挡显露、镜面/透射必须有明确策略；只用 camera motion 不足以覆盖动态对象。

先形成可验证的 Vulkan 通用重建接口，再评估 NRD / DLSS-RR。依赖、许可证、feature、设备支持、DLL/SDK 分发与 Android 回退单独设计，feature 名存在不等于已接 SDK。重建与路径积分质量分别测量。

稀疏采样和动态分辨率仅在历史重建、disocclusion 与有效 sample count 正确后启用。OMM 只在剩余 MASK alpha 成本明显且扩展/工具链可用时评估；SER/RT pipeline 仅在 profiler 显示重排收益且 Vulkan/Slang 支持明确后立项；异步 compute 需要实测可重叠工作、队列同步与内存预算。均保留基础 RayQuery 路径。

## 13. 实施拆分与依赖

以下是建议 review 单元；每个默认 <500 行，超过则按数据契约、实现、测试继续拆，禁止整个规格合成一个大 PR。进入 master 每任务独立分支/PR；本规格不授权提交、推送或发布。

| 顺序 | 交付单元 | 主要所有权/入口 | 前置 | 验收 |
|---|---|---|---|---|
| 1 | 基准 manifest、区间计时与诊断计数 | `gpu_timer.rs`、render graph 报告、`prism-app` bench、Sponza 文档/脚本 | 无 | 可重复基线、计时无漏项 |
| 2 | alpha/RMAT/API 契约与版本方案 | asset/cooker 格式文档、material schema | 1 | 兼容与迁移规则可 review |
| 3 | 导入/烘焙/运行时 alpha 参数贯通 | `prism-cooker`、`prism-asset`、engine asset、material manager | 2 | roundtrip 与旧包测试 |
| 4 | opaque geometry、精简 alpha、visibility flags | `acceleration_structure.rs`、`bake_common.rs`、PT helpers | 3 | 材质矩阵画面与计数 |
| 5 | 共享输运参考变体与能量测试 | `path_integrator.slang`、PT host、微场景 | 1 | 参考收敛与解析测试 |
| 6 | SHaRC 量纲、权重、返回值、重复计能修复 | `pt_render.slang`、SHaRC common | 5 | 传播和缓存 ROI 验收 |
| 7 | 独立训练 shader 与无训练消费变体 | SHaRC shaders、compile/reflection/bindings | 6 | 三变体正确编译、资源布局验证 |
| 8 | 图内 train/resolve/consume 与历史生命周期 | `sharc.rs`、`graph_renderer.rs`、graph settings | 7 | 同步 validation、多帧/reset 测试 |
| 9 | 单灯快速路径与 reservoir 历史规则 | PT light helpers、历史状态 | 4、5 | 单灯零 reservoir、多灯无偏差 |
| 10 | ray cones 与 mip 链核实 | material helpers、PT state、texture pipeline | 5 | LOD/画质/带宽报告 |
| 11 | AS 共享和动态失效 | mesh manager、AS、geometry sync | 4 | 静态复用、动态/重载验证 |
| 12 | 线程组、格式和 CPU 热点实验 | PT、输出资源、frame preparation | 8–11 | 仅保留有效收益 |
| 13 | 对照报告与权威文档同步 | `docs/DESIGN.md`、移动 GI 文档、Sponza README | 12 | 完整基线与限制交接 |
| 后续 | 通用重建/SDK/硬件优化各自 spec | render traits、平台 capability、host | 基础闭环 | 独立审批与验收 |

新公开 API、资源格式或依赖先落对应设计文档再编码。不要先实现全部功能后补格式说明。渲染设置采用现有 INI/engine 映射，诊断参数不无限扩张成公开 API。

## 14. 质量门槛、验证与回滚

建议初始性能门槛：相同质量下，最终 Sponza 暖缓存 p50 GPU 时间相对已记录基线降低至少 20%，p95 不回退超过 3%；这是工程目标而非已测结论。每项优化收益须大于重复运行噪声，且硬件、原始/等质量模式分别报告。正确性修复如增加成本应单列，不能为达标撤销正确结果。

SHaRC 训练+resolve+消费总时间与 cache-off 比较，同时报告图像误差和更深反弹收益；不能仅比较 consume 时间。暂未达性能目标时保留正确性修复，记录残余热点和下一阶段，不通过降低光照复杂度伪造收益。

实现阶段按改动执行：

```bash
python scripts/pr_ready.py
cargo test -p prism-render --features rtxgi
cargo test -p prism-asset -p prism-cooker
cargo check -p prism-render --no-default-features
cargo check --manifest-path projects/sponza/Cargo.toml --release
```

独立 editor/template workspace 按涉及范围验证；玩家依赖预算用仓库现有脚本检查。shader 修改必须使用仓库钉定 Slang 2026.21，更新 `.spv`，反射有变化按 `compile.sh --write-reflection` 与 shader-bindgen 正式流程生成，运行绑定布局/资源预算验证；禁止手改 JSON 或 push struct。

GPU 验证覆盖 validation、同步、resize、退出、场景切换、光栅↔PT、GI 开关、无 RT 回退；没有实际 Android 真机证据不得声称移动端已验证。截图工具截图不能替代线性 HDR 的质量度量。

每个单元交接必须包含：实现/未实现、入口文件、命令及结果、运行 manifest、before/after GPU 与图像证据、已知风险。基础修复保留可诊断开关；格式升级保留明确 reader/migration 策略，性能候选可独立 revert。GPU 资源释放依赖完成信号；回滚不能留下无效 descriptor、旧缓存 ABI 或过期 `.spv`。

## 15. 当前交付与下一步

已按第 13 节的拆分落地以下单元（本地工作树，未提交）：

| 单元 | 状态 | 落地内容与证据 |
|---|---|---|
| 1 | 已实现，缺基线数据 | `gpu_timer.rs` 增加整帧区间（span query、`frame_or_total_ms`、`unmeasured_ms`），HUD 与 bench 改读该值，新增 HUD/区间测试 |
| 2、3 | 已实现 | RMAT v2 携带 `alpha_mode` + `alpha_cutoff`；导入、cooker、asset、engine 上传、`GpuMaterial`（112 字节）全链贯通；旧版本解码分支已删除 |
| 4 | 已实现 | `BlasBuildParams.opaque` 驱动 `GeometryFlagsKHR::OPAQUE`；PT 用 `mat_alpha_pass` 使用真实阈值；阴影射线加 `ACCEPT_FIRST_HIT_AND_END_SEARCH` |
| 6 | 已实现 | 缓存查询提前到 NEE 之前，命中即终止该段；`SharcUpdateHit` 返回 false 终止、按段调用 `SharcSetThroughput` |
| 9 | 已实现 | 太阳单光源时 `restir_on = false`：主命中直接采样太阳，`prevReservoir` / `currReservoir`（含天空像素的清零写）完全不访问；多光源仍走原 ReSTIR |
| 5 | 已实现 | `pt_reference.slang`（`PT_MODE=1`）：无缓存、无 ReSTIR、无 firefly clamp、均匀灯采样，线性 HDR 输出；`PathTracePass::set_variant` / `GraphRenderer::set_pt_variant` 可选择；离屏场景已实际 dispatch，参考平均亮度与消费路径相差 < 20% 的断言通过。线性 HDR 回读：`RenderGraph::request_hdr_readback` / `take_hdr_readback` 在 tonemap 前抓 `PT_COLOR_H`（`hdr_readback.rs`，`HdrReadback` 提供 `pixel`/`mean_rgb`/`max_rgb`/`has_non_finite`），`GraphRenderer` 同名方法转发。解析微场景套件 `crates/prism-render/tests/pt_analytic_gpu.rs` 6 个用例全过 |
| 7 | shader 三变体已实现，图内接线未做 | `pt_render.slang` 用 `PT_MODE` 派生消费/参考/训练；`pt_sharc_train.slang`（`PT_MODE=2`）只写入缓存、不查询、不写屏幕输出；三个 `.spv` 与反射 JSON 均已生成，push 常量均为 176 字节、绑定表一致（单元测试断言）。训练变体的命中记录：`SharcHitData` 的 `positionWorld` / `normalWorld` 原来只在 `if (sharc_on)` 里填，而训练变体 `sharc_on=false`（`PT_MODE_TRAIN` 只打开 `sharc_update`），于是 `SharcUpdateHit` 拿未初始化的坐标做法线键 —— 沉积全进随机体素、消费端永远命中不了。现改为 `if (sharc_on || sharc_update)` 填充，训练与查询用同一个键 |
| 8 | 已实现 | 顺序：每帧在 `GraphRenderer` 内按 `SharcTrain`（`PathTracePass::dispatch_training`，1/downscale、ceil 取整）→ 缓存缓冲交接 → barrier → `SharcResolve` → barrier → 消费路径执行；消费路径不再训练（`sharc_flags` 的 update 位恒为 0）。两个阶段各有独立 GPU 区间名 `SharcTrain` / `SharcResolve`。生命周期：世界缓存与屏幕历史分离——`resize_offscreen` / `recreate_swapchain` 只清屏幕历史，世界缓存保留靠重投影；`SharcInvalidation { geometry, lights, sky }` 记录需要整缓存作废的原因（`invalidate_pt_geometry`、`set_environment`、`release_assets` 释放过资源、帧脏计划里的 `lights_dirty`/`sky_dirty`/`mesh_releases`/`material_releases`），voxel 尺度变化经 `sharc_cache_scene_scale` 纪元单独置位；原因在 SHaRC 空闲帧跨帧保留，回到 PathTrace 的第一帧才消费并清空。曝光与相机移动都不清世界缓存 |
| 10、11、12、13 | 未开始 | ray cones、AS 共享、线程组与格式实验、对照报告 |

验证记录（2026-11-13）：

- `bash assets/shaders/compile.sh`：反射与已提交 JSON 一致，spirv-val 全部通过；新增 `pt_reference.comp.spv`、`pt_sharc_train.comp.spv` 及两份反射 JSON，`shader-bindgen` 生成对应 Rust 模块。
- `cargo test -p prism-render --lib`：288 通过；`cargo test -p prism-asset -p prism-cooker`：139 / 204 通过。
- `cargo test -p prism-render --features rtxgi --test rtxgi_sponza_golden`：通过。rtxgi 构建下断言 GPU 区间里存在 `SharcTrain`，证明训练是独立 dispatch 且与消费分离。原来的断言到此为止，但「区间被打开」不等于「dispatch 真的发生」——`GraphRenderer::execute` 无条件 `begin_pass`，而区间里的 `PathTracePass::dispatch_training` 命中任意一条前置条件就静默返回。现已改为断言渲染器自己记的 `sharc_train_dispatches()` / `sharc_resolve_dispatches()` 大于 0（`dispatch_training` 改为返回 `anyhow::Result<bool>`，只有真的记录了 `record_dispatch` 才返回 `true`）。
- `cargo test -p prism-render --test rtxgi_sponza_golden`（默认特性，rtxgi 关闭）：通过；SHaRC 相关断言按特性跳过，参考变体与训练变体的渲染仍被执行。
- 单元 9 的等值性 A/B：临时把 `restir_on` 固定为 true 重跑同一离屏场景，与快路径图像相比平均亮度 2.621 对 2.631（差 0.38%），逐像素差异来自两条路径消耗随机数的顺序不同（1 spp 噪声），未出现整幅变暗或加倍的加性偏差。
- SHaRC 训练键修复的实证（`assets/shaders/slang/pt_render.slang` 的 `sharc_hit` 初始化）：同一离屏场景、同一相机连跑 16 帧，`gi_mode = Off` 与 `gi_mode = Sharc` 逐像素比较 —— 修复前 4096 像素全部逐位相同（缓存零命中），修复后 `sharc_scene_scale = 5` 有 67 个像素被缓存改写、`scene_scale = 50`（出厂值）有 6 个。这也解释了为什么只靠 GPU 计时里的 `SharcTrain` / `SharcResolve` 区间名不能证明训练真的发生：区间被打开时 `dispatch_training` 仍可能被守卫静默早退。
- SHaRC dispatch 计数（消除空转断言）：`PathTracePass::dispatch_training` 现在返回 `anyhow::Result<bool>`，`GraphRenderer` 据此维护 `sharc_train_dispatches` / `sharc_resolve_dispatches` 两个 `u64` 计数器（带 `#[cfg(feature = "rtxgi")]` 的 `GraphRenderer::sharc_train_dispatches()` / `sharc_resolve_dispatches()`）。`pt_analytic_gpu.rs` 的缓存用例里加了两向对照：`gi_mode = Off` 的 16 帧必须让两个计数器保持在 0（渲染器刚建好、还没进过 SHaRC 分支，所以这个 0 是精确值；它同时排除了「计数器恒为 1」这类坏法），`gi_mode = Sharc` 的 16 帧必须让两者都递增。`rtxgi_sponza_golden.rs` 的四帧累积后同样断言两者都大于 0。
- 单元 5 的解析微场景 `cargo test -p prism-render --features rtxgi --test pt_analytic_gpu`：6 个用例全过（白炉线性：0.25 与 0.5 两档各自整幅均匀且均值比 2.000；两段 throughput：`(albedo + 0.0256) * 0.375` 解析值与中心像素相对误差 < 5%，bounces 2 与 4 结果一致；纯发射：中心等于材质 `emissive * emissive_strength` 且三个通道相同、盒外为 0；单方向光：`(albedo/π + 0.1024·0.25/π)` 相对误差 < 5%，强度 1 与 2 的像素比 2.00；退化空 hash：`sharc_capacity = 0` 下无 NaN/Inf 且无能量放大；缓存能量：`gi_mode` 关/开两跑整幅平均亮度差 < 5%，且至少一个像素被缓存改变，证明查询真的命中）。它们走的是 `take_hdr_readback` 的线性 HDR 而不是 tonemap 后的 8-bit 截图 —— 后者会把 0.25/0.5 压成 0.19/0.31。
- 单元 8 的生命周期测试 `cargo test -p prism-render --features rtxgi --test sharc_lifecycle_gpu`：5 个用例全过，覆盖 resize 不失效（`resize_offscreen(192,160)` 前后清空次数不变）、灯光变化清一次且不重复、换环境清一次、voxel 尺度变化清一次、以及「光栅帧里登记的原因跨帧保留、回到 PathTrace 首帧才消费」。`cargo test -p prism-render --lib`：292 通过；`cargo test -p prism-engine --lib dirty_router`：7 通过（`light_view_proj` 不再参与光照脏判定，因为它由相机 eye 派生，否则玩家走路会每帧清空世界缓存）。
- `python scripts/pr_ready.py --quick`（单元 8 完成时）：与本工作线无关的既有失败有 `editor/crates/prism-editor-core/src/build.rs` 等三处格式、`crates/prism-cooker/src/aseprite.rs` 的 `useless_vec` clippy；当时着色器绑定与预算（17 个模块）通过。此后并行的 TAA-on-PT 工作线给 `pt_render` / `pt_reference` / `pt_sharc_train` 加了 `guides` UBO 与 `guideDepth` / `guideVelocity` 采样图，shader budget 现在报 9 条违规（`sampled_images 6 > 4`、`uniform_buffers 1 > 0`、`descriptors 21 > 18`），需要该工作线决定是否上调 `assets/shaders/budgets.json`。

未完成的人工验收：Sponza 的 HUD GPU 时间、MASK 镂空外观、SHaRC 画面与单光源画面的目视对比。单元 6、8 与 9 改变了可见结果，必须在 `bash scripts/dev.sh sponza --release` 下由人确认；本次没有采集 before/after GPU 毫秒数，性能收益尚未量化。

下一步：把 `SharcTrain` / `SharcResolve` 收进 `RenderPassNode`（目前仍在 `GraphRenderer::execute` 里手工编排，图外执行）。第 5 项要求的解析微场景已由 `crates/prism-render/tests/pt_analytic_gpu.rs` 覆盖。图内接线的硬阻塞已探明：`GraphResources` 只装 Vulkan 句柄与 `u32` 标量（`params`），`RenderPassNode::execute` 也拿不到别的 pass，所以「SharcTrain pass 直接调 `PathTracePass::dispatch_training`」无法靠现有图内交接实现，要么给 `GraphResources` 加任意类型槽，要么让训练/解析各自变成独立的前向/后向依赖链上的真实 pass。
