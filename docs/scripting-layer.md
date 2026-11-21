# 脚本层（`scripting`）

> **状态（2026-11-21）：设计契约，未实现。** 本文只固定接缝与分期判据，
> 不承诺任何语言落地的排期，也不代表相关 API 已经改动。

## 1. 决策与范围

**目标形态**：用户项目的 gameplay 可以由脚本编写，脚本与 Rust 项目模块共用**同一条宿主边界**，
而不是各自拥有一套跨语言协议。

**为什么现在写这份文档**：#1253 的 A/B 决策讨论过"用户不装 Rust 工具链能否构建独立播放器"，
结论是"除非引擎嵌入脚本运行时、且用户项目也用脚本，否则不考虑"。本文把那个前置条件
具体化：脚本层长什么样、接缝在哪、代价是多少、什么时候才值得做。

**非目标**：

- 不在本文决定 Lua 与 C# 的先后之外的排期。
- 不引入新的跨语言 ABI（见 §2）。
- 不改变 1.0 的判定标准（见 §9）。
- 不承诺 Android 上的 C# 支持（见 §7，这是一个独立的 go/no-go）。

## 2. 为什么脚本不是新 ABI

`HostApi`（`crates/prism-project-api/src/lib.rs:573`）已经是一张完整的脚本宿主函数表。
它今天的消费者是 Rust 预览模块，但它需要的表达能力与脚本运行时完全一致：

| 脚本需要的能力 | `HostApi` 字段 |
|---|---|
| 创建 / 销毁实体 | `spawn_entity` / `despawn_entity` / `is_alive` |
| 按类型名枚举实体 | `query_entities(type_name, len)` |
| 读写组件 | `get_component` / `set_component` / `remove_component` |
| 日志 | `log` |
| 资源读取 | `asset_exists` / `read_asset_file` |
| 存档 | `save_value` / `load_value` / `save_scene_state` |
| 场景切换 | `load_scene` / `unload_scene` |
| 物理查询 | `raycast` / `query_colliders2d` |
| 音频 | `play_sound` / `stop_sound` |
| UI 绘制 | `draw_quad` |

**决定性的一点**：组件读写按**类型名字符串 + RTTIBin 字节流**传递
（`GetComponent` / `SetComponent` 的签名见 `crates/prism-project-api/src/lib.rs:509`），
不是 Rust 泛型。因此脚本侧**不需要为每个组件生成绑定**——组件集合在运行期由
`ComponentRegistry` 决定，脚本看到的是字段名和值，不是类型。

这条性质必须保持。任何让组件读写变成"每加一个组件就要重新生成一份绑定"的设计，
都会把脚本层的维护成本变成线性增长，直接否决。

## 3. 反射侧已经够用

`prism-reflect` 提供的动态访问路径正是脚本需要的：

- `Value` / `Object`（`crates/prism-reflect/src/lib.rs:96,214`）：字段名 → 值的通用表示；
- `TypeRegistry::from_object` / `to_object`（`crates/prism-reflect/src/registry.rs:32,138`）：
  在动态对象与具体 Rust 类型之间转换；
- `set_field` 拒绝 `read_only` 字段，`set_field_force` 供反序列化使用；
- 字段名就是 `.scene.json` 里的字段名（`#[reflect(rename = ...)]` 之后的），
  所以脚本 API 与场景文档、检查器面板三者使用同一套名字。

RTTIBin 格式在 [`REFLECT.md`](REFLECT.md) §RTTIBin 格式（v1）有完整规范，
脚本侧实现一个读取器是有限工作量，且不需要引擎侧配合。

## 4. 唯一缺口：动态组件访问是私有的

`GameWorld` 的公开 API 只有泛型版本：

```rust
pub fn get<T: Component + Reflect + Clone>(&self, entity: EntityHandle) -> Option<T>
pub fn set<T: Component + Reflect>(&mut self, entity: EntityHandle, value: T) -> bool
```

而按类型名的动态路径 `fn get(&self, entity, type_name) -> Option<Vec<u8>>`（`crates/prism-game-api/src/lib.rs:1138`）
与 `fn set(&self, entity, bytes)`（`:1161`）是**私有的**。

脚本层必须走动态路径。因此**最小且语言无关的准备**是把动态访问提升为公开 API，
且返回 `Object` 而不是裸字节：

```rust
pub fn get_object(&self, entity: EntityHandle, type_name: &str) -> Option<Object>
pub fn set_object(&mut self, entity: EntityHandle, object: &Object) -> bool
```

要求：

1. **两种后端都实现**（`Backend::Host` 与 `Backend::Plugin`）。Host 后端直接借用
   `World`，Plugin 后端走 `HostApi`；两者签名相同是 `prism-game-api` 的既有契约。
2. **不新增依赖**，不改变现有泛型 API 的行为。
3. 字节与 `Object` 之间的编解码在 `prism-game-api` 内部完成，脚本层不接触 RTTIBin。

这一步无论最终选 Lua、C# 还是都不做，都不会白做——它是"脚本能否复用现有边界"的判据本身。

## 5. Lua 轨道

**形态**：同进程 VM，编译期 feature 砖块，`Subsystem::Script`。
`Subsystem` 已有"占位但未实现"的先例（`Network` / `AI` 在
`crates/prism-app/src/app/mod.rs:595` 走 `warn` 分支），可以沿用同一种写法。

**工具链约束（已实测修正）**：玩家依赖树**已经**包含 C 工具链——
`cc` 与 `pkg-config` 经 `zstd-sys`（`prism-asset` 的 `.pak` 解压）进入
`cargo tree -p prism-app -e normal,build`。因此"引入 C 实现的 Lua VM 会新增编译器依赖"
这个担心不成立；NDK 侧的 clang 本来就要为 zstd 准备。

由此选型：

| 方案 | 结论 |
|---|---|
| `mlua` + vendored PUC Lua 5.4 | **推荐**。工具链无新增；Lua 5.4 语义稳定；Android 走 NDK clang |
| `mlua` + LuaJIT | 不推荐。aarch64 Android 上 JIT 受限且需要额外后端，收益不匹配 |
| 纯 Rust VM（`piccolo`） | 备选。省掉 C，但标准库覆盖与成熟度不足，作为 fallback 而非首选 |

**热重载**：这是 Lua 相对 Rust 预览模块的真正收益。`docs/editor-egui.md` §3.2 现在写着
"没有脚本层可以迁移状态，所以文案统一用 reload，不要用 hot reload"；脚本层存在之后，
编辑器可以做到**状态保留的脚本重载**，而不是整模块替换 + 项目侧静态状态归零。
这条收益要单独验收，不能默认"有脚本就等于有热重载"。

## 6. C# 轨道

**产品要求**（来自 owner）：不要过时的 Mono；要最新语言特性；编辑器下方法级热重载
而不是完整域重载；构建时可选 NativeAOT。

**结论：这四点相互自洽，且只有一种切分方式。**

| 要求 | 事实 | 落地形态 |
|---|---|---|
| 不用 Mono | 现代路径是从原生代码托管 .NET 运行时（`nethost` / `hostfxr`） | 编辑器内 CoreCLR hosting |
| 方法级热重载 | CoreCLR 的 Hot Reload（`MetadataUpdateHandler`）能改方法体、加方法/类型；只有 rude edits 需要重启 | 编辑器内 JIT 运行时 |
| 构建可选 NativeAOT | NativeAOT 无运行时 codegen，**原理上无法做方法级热重载** | 发布产物走 NativeAOT |
| 最新特性 | 跟随当前 LTS / STS 的 `net` TFM | 两者同源，仅发布模式不同 |

所以切分是**强制的**，不是偏好：编辑器 = CoreCLR + JIT + 热重载；发布 = NativeAOT。
任何试图用同一份产物同时满足两者的方案都不成立。

**两个必须提前承认的风险**：

1. **Android 没有干净的现代路径**。官方 NativeAOT 平台表（.NET 8 与 .NET 9+ 两栏一致）
   把 Android 标为 *"Experimental, no built-in Java interop"*。而本仓库的 Android 契约是
   单体 `.so` + GameActivity（见 [`project-module-architecture.md`](project-module-architecture.md) §7）。
   **这是独立决策，不能顺带承诺。**
2. **体积**。NativeAOT 最小产物是 MB 量级，而引擎 `dist` 档位是 2.89–4.47 MB
   （[DESIGN.md](DESIGN.md) §12.4）。C# 只能是**默认关的 opt-in 砖块**，
   且必须进 §12.4 体积账本实测，不能估。

**工程面**集中在两处，都不是架构风险：用 `LibraryImport` source-gen 绑定 `HostApi`
（结构体必须 blittable，`InputSnapshot` 里的 `bool` 要按 `U1` 编组）；
在 C# 侧实现 RTTIBin 读取器（格式见 [`REFLECT.md`](REFLECT.md)）。

## 7. 平台与体积约束

- 脚本运行时是**砖块**：`script-lua` / `script-csharp`，默认关，进
  [DESIGN.md](DESIGN.md) §12.2 表格、§12.4 体积账本，以及 `scripts/check_features.py`
  的最小组合矩阵（§12.3 硬规则 1）。
- 关掉时必须在启动/使用时 `log::warn!` 说明缺哪个 feature（§12.3 硬规则 3），
  与 `Subsystem::Audio` 的既有做法一致。
- 脚本层**不进 player 链的 UI 依赖**：它是 `prism-app` 的子系统，
  `scripts/check_deps.py` 的黑名单与 `cargo tree | grep -i egui` 仍须为空。

## 8. 分期与 go/no-go

| 阶段 | 内容 | 判据 |
|---|---|---|
| **P0 接缝准备** | `GameWorld::get_object` / `set_object` 公开，两种后端都实现 | 不涉及任何脚本依赖；Lua 与 C# 都能复用同一条通道 |
| **P1 Lua** | `script-lua` 砖块、`Subsystem::Script`、`HostApi` 的 Lua 绑定、状态保留重载 | 一个真实项目用 Lua 写出可玩的 gameplay；体积进账本 |
| **P2 C# 编辑器** | CoreCLR hosting、`HostApi` 绑定、`MetadataUpdateHandler` 热重载 | 改一个方法体不需要重启编辑器 |
| **P3 C# 发布** | NativeAOT 产物 + 体积实测 | 体积可接受才进 `dist` 档位表 |
| **P4 Android C#** | 独立 go/no-go | 默认结论是 **No**，除非官方支持从实验性转正 |

**P0 可以立刻做**，且与语言选型无关。P1 之后再决定 P2 的时间点。

## 9. 与 1.0 的关系

[`versioning.md`](versioning.md) §1 规定 1.0 的门槛是**公共游戏 API 稳定**
（`prism-game-api` / `prism-project-api` 不再频繁变化）。

脚本层会显著扩张公共 API 面。因此只有两条合法路线：

1. 在 1.0 之前定下来，一起冻结；
2. 明确把脚本层排除在 1.0 范围之外，1.0 之后作为破坏性变更引入。

按 AGENTS.md §3 的零兼容负担原则，**不能先出一个版本再改**。

## 10. 参考

- `crates/prism-project-api/src/lib.rs` — `HostApi` 表、`GetComponent` / `SetComponent` 签名、能力位
- `crates/prism-game-api/src/lib.rs` — `GameWorld` 的 host / plugin 双后端，动态访问现状
- `crates/prism-reflect/src/` — `Value` / `Object` / `TypeRegistry`
- `crates/prism-app/src/app/mod.rs` — `Subsystem` 枚举与占位分支
- [`REFLECT.md`](REFLECT.md) — RTTIBin v1 格式
- [`project-module-architecture.md`](project-module-architecture.md) — 宿主边界、P3/P4 状态
- [`DESIGN.md`](DESIGN.md) §12 — 砖块机制与体积账本
