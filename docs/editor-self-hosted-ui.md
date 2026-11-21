# Prisma 自举式编辑器 UI 架构设计 Spec

> **目标**：在不破坏 [AGENTS.md](../AGENTS.md) 依赖边界的前提下，基于 Prisma 现有的 `prism-ecs`、`prism-reflect`、`prism-render`（`ui_overlay` / 离屏 `VkImage`）与 `prism-editor-core`，设计一套**无继承、无 `Rc<RefCell<T>>`、高度组合复用**的保留模式 UI 内核与编辑器架构。
>
> **UI 权威归属已定为 ECS**（§9）。§2–§4 描述的 `prism-ui` 树/布局/绘制内核在该决策下**不再作为独立运行时**，只保留声明式 builder 与交互状态机的职责；阅读时以 §9 为准。

---

## 1. 总体分层与依赖边界

遵守玩家链零 UI / 零编辑器依赖的红线（`template → prism-app → prism-engine → {scene, ecs, render, asset, physics2d}`），并采用**“编辑器即游戏”**模型，将自举编辑器作为独立项目落在 `projects/editor/`：

```text
projects/editor/             # 独立 workspace（仿 projects/template），"编辑器即游戏"
├── Cargo.toml               # 独立 [workspace]，通过 prism-app + prism-engine 启动
├── assets/                  # 编辑器自身图标、字体、主题、默认布局场景
└── crates/
    ├── prism-ui             # 可复用保留模式 UI 内核（SlotMap 树 + 行为组合 + DrawList）
    └── editor-app           # 绑定 prism-editor-core + prism-editor-api + prism-runtime-host

既有共享底座（直接复用）：
├── editor/crates/prism-editor-core   # UI 无关业务核心（dock / undo / textinput / theme / keys）
├── crates/prism-editor-api           # 反射 schema / reflect_edit / property_drawer
├── crates/prism-runtime-host         # 加载目标游戏项目的 preview-module cdylib（RTTIBin 跨 ABI）
└── crates/prism-render               # UiOverlayHost / 离屏 VkImage / Gizmo
```

- **`projects/editor` 的天然优势**：
  1. **零污染隔离**：仿照 [projects/template/Cargo.toml](../projects/template/Cargo.toml) 使用独立 `[workspace]`，不进入根 workspace，玩家工程零感知。
  2. **真·引擎自举**：直接走 `prism-app` + `prism-engine` 主循环与渲染管线，天然支持 `prism-runtime-host` 加载被编辑游戏的 `preview-module`（`.so`/`.dll`）进行 Play/Preview。
  3. **非运行时依赖不设限**：因为 `projects/editor` 独立于玩家运行时链，可放心引入成熟生态库替代手写桌面基础设施（如布局、文本排版、剪贴板、系统文件框），把精力聚焦在编辑器业务逻辑与可复用控件组合上：
     * **`taffy`**（或轻量 Flexbox 内核）：驱动分屏 Docking、Inspector 行列与自适应面板。
     * **`slotmap`**：`SlotMap` + `SecondaryMap` 扁平存储与正交行为表，零 `Rc<RefCell<T>>`。
     * **`cosmic-text`**：多行文本排版、双向文字、字体回退与精确光标 HitTest（配合 `prism-render` 图集缓存上传字形）。
     * **`arboard` / `rfd` / `notify`**：跨平台剪贴板、原生文件对话框、资产/动态库热重载文件监听。
  4. **`prism-ui` 的定位**：只输出平台无关的 `UiDrawList`（矩形、圆角边框、裁剪区、字形图集、纹理句柄 `UiTextureId`），通过 `prism-render` 的 `UiOverlay` / `SwapchainOverlay`（见 [ui_host.rs](../crates/prism-render/src/graph_renderer/ui_host.rs)）完成合批上屏。

---

## 2. 核心数据模型：SlotMap 扁平树 + 稳定 NodeKey

用整数句柄代替指针引用，彻底消除自引用生命周期与借用冲突：

```rust
slotmap::new_key_type! { pub struct NodeId; }

pub struct UiTree {
    nodes: SlotMap<NodeId, UiNode>,
    key_to_id: HashMap<NodeKey, NodeId>, // 稳定语义 ID（如 "field:3:Transform:position[1]"）
    // 正交行为表（SecondaryMap<NodeId, T>）...
}
```

### 关键设计点
1. **双重用途的 `NodeKey`**：编辑器中每个节点绑定语义化的 `NodeKey`（如 `panel:hierarchy`、`entity:3`、`field:3:Transform:position[1]`），既用于**帧间状态保留**（动画插值、滚动偏移、折叠状态、文本光标、拖拽基准），也直接作为 `prism-editor-mcp` 自动化协议（`ui_tree.rs`）的节点 ID，避免维护两套树。
2. **单向借用流**：事件分发（HitTest → Capture → Bubble）与布局计算只读写 `UiTree` 内部数组；业务回调通过**事件队列 `Vec<UiAction>`** 吐出，不在树遍历过程中持有可变闭包回调引用，彻底杜绝双重可变借用。

---

## 3. 组合优先：正交行为组件（Behaviors）取代控件继承

将**外观**、**交互**、**数据编辑**拆分为正交的轻量数据结构，挂载在独立类型表（`SecondaryMap<NodeId, T>`）中：

| 正交组件 (`SecondaryMap<NodeId, T>`) | 职责 | 复用的场景 |
|---|---|---|
| `VisualBox` | 背景色、边框、圆角、阴影，按 `InteractionState` 查主题令牌 | 几乎所有容器、按钮、输入框、标签页 |
| `Interactable` | 点击/双击判定、开关状态、按键激活、指针捕获 | 按钮、树节点、菜单项、Splitter、Gizmo 面板 |
| `Focusable` | Tab 序、焦点环绘制、快捷键作用域隔离（输入文本时屏蔽视口 WASD） | 输入框、数值框、树列表、面板容器 |
| `TextEditable` | 光标、选区、撤销、剪贴板、IME 预编辑区 | 搜索框、重命名框、表达式输入、数值内联编辑 |
| `DraggableValue` | 水平拖拽改值（`Shift` 精细、`Ctrl` 步进吸附、边界 clamp） | `f32`/`i32` 拖拽框、Slider 滑块、时间轴游标、Splitter |
| `Scrollable` | 视口裁剪、滚轮偏移、虚拟化可视区间 `[start..end)` 计算 | Hierarchy、Asset Browser、Console、Inspector、下拉列表 |
| `DragPayload` / `DropTarget` | 统一的拖拽载荷（`Entity` / `AssetPath` / `DockTab`）发起与接收 | 层级拖拽改父级、资产拖入视口/字段、Dock 标签页拖放 |

---

## 4. 可复用控件库与组合矩阵

通过**原语（Primitives）→ 交互原子（Atoms）→ 复合容器（Molecules）**三层组合，最大化消除重复代码：

### 4.1 原语层
* `row` / `col` / `spacer` / `divider`
* `label` / `muted_label`
* `viewport(UiTextureId)`（零拷贝嵌入离屏 `VkImageView`）
* `canvas(Vec<UiDrawCommand>)`

### 4.2 交互原子层（同一内核，参数化变体）
1. **`button_core`（统一所有按钮类控件）**：组合 `ElementBox` + `VisualBox` + `Interactable` + `Focusable`，通过 `toggle: Option<bool>` 覆盖普通按钮、工具栏图标按钮、开关按钮（Toggle）、Tab 标签页与菜单项。
2. **`text_field`**：组合 `ElementBox` + `VisualBox` + `Focusable` + `TextEditable`。
3. **`numeric_field` / `vec3_field`**：组合 `VisualBox` + `DraggableValue` + 懒切换 `TextEditable`，单一实现覆盖 `f32`/`i32` 数值框、Slider、`Vec2/3/4` 与时间轴游标。

### 4.3 复合容器层
1. **`virtual_list`**：组合 `Scrollable` + 可视区间裁剪，直接驱动 Console、Asset Browser、Command Palette。
2. **`tree_view`**：直接建立在 `virtual_list` 之上，仅追加 `Indent(depth)` + `ExpandToggle(toggle_button)`，驱动 Hierarchy、Asset 目录树、Prefab 结构树。

---

## 5. 数据驱动的 Inspector 与 Undo/Redo 组合

依托 [crates/prism-editor-api](../crates/prism-editor-api/src/lib.rs)（`schema`、`property_drawer`、`reflect_edit`）与 [crates/prism-reflect](../crates/prism-reflect/src/lib.rs)：

1. **单一查表映射**：遍历实体反射组件的 `FieldSchema` 与 `PropertyAttribute`，统一映射为 `numeric_field`、`vec3_field`、`toggle_button`、`text_field`、`button`（枚举选择）。
2. **差分 Undo/Redo**：控件层在 `ValueCommitted` 时发出 `FieldEdit { entity_id, component, field, axis, old_value, new_value }` 压入 `UndoStack`。

---

## 6. 世界隔离与 Render Graph 多视口管线

### 6.1 三世界隔离模型
* **`EditorWorld`**：持有编辑器的 `UiTree`、编辑器相机、选中集合与 Gizmo 状态。
* **`SceneWorld`**：当前打开的关卡文档内存镜像（仅运行 `Transform` 矩阵传播与渲染提取，**跳过游戏玩法/物理/AI 系统**）。
* **`PlayWorld`**：点击 Play 时，通过 `prism-scene` / `RTTIBin` 快照从 `SceneWorld` 重建独立 `World` 并激活完整游戏系统；Stop 时直接 `drop(play_world)`。

### 6.2 Render Graph 单帧管线
```text
[SceneOffscreenPass] ──► OffscreenColor (VkImage)
                                │
[GizmoOverlayPass]  ────────────┼──► 合成到同一 OffscreenColor
                                │
[UiCompositePass]   ◄───────────┘ (ImageViewport 直接采样 VkImageView，零拷贝)
        │
        └──► Swapchain Present（主窗口 / 未来多窗口扩展的独立 VkSurfaceKHR）
```

---

## 7. 渐进式迁移路径

> **已被 §9.8 取代。** 本节保留为原始提案记录；在 ECS 权威（§9.1）下，实际分期以 §9.8 为准——
> 引擎 UI 能力补齐（Phase 0–1）必须先于 `prism-ui` 收敛（Phase 2）。

1. **Phase 1 — 渲染底座对齐**：扩展 `UiOverlay` 支持外部注册的 `OffscreenColor` `VkImageView`（复用 `UiOverlayHost`）。
2. **Phase 2 — `prism-ui` 内核与原语**：实现 `UiTree`、正交行为表、`button / text_field / numeric_field / virtual_list / tree_view` 及单元测试。
3. **Phase 3 — `projects/editor` 跑通主循环**：以标准 Prisma 游戏入口启动 `editor-app`，对接 `prism-editor-core`、`prism-editor-api` 与 `prism-runtime-host`。
4. **Phase 4 — 多窗口扩展**：扩展 `prism-platform` 与 `prism-render` 支持同一 `VkDevice` 下的多 `winit::Window` / `Swapchain` 管理。

---

## 8. 现代化引擎 UI API 与模块化可选特性

延续 [crates/prism-render/Cargo.toml](../crates/prism-render/Cargo.toml) 的积木式 Feature 哲学，高阶视觉与动效全部做成零入侵的可选 Cargo Feature：

| Feature 开关 | 能力说明 | 底层实现与优势 |
|---|---|---|
| **`ui-blur`** | 磨砂玻璃 / Acrylic 背景（`.backdrop_blur(...)`） | Render Graph 内对背景纹理做 Dual-Kawase 降采样模糊 + 微噪点防色带 |
| **`ui-tween`** | 声明式 Tween 与物理 Spring 弹簧动画 | 依托 `SlotMap` + 稳定 `NodeKey` 做跨帧属性插值 |
| **`ui-effects`** | 解析 SDF 软阴影、内/外发光与自定义 `.slang` UI 材质 | 单片元 SDF 抗锯齿，支持绑定自定义 Slang 着色器 |
| **`ui-desktop`** | 桌面全功能（多行排版、IME、剪贴板、虚拟列表、Docking） | 仅由 `projects/editor` 等桌面工具开启，不影响极简玩家包 |

---

## 9. UI 权威归属：ECS（决策记录与补充设计）

> 本节补入 [issue #1254](https://github.com/prisma-rust/prisma/issues/1254) 的 A/B 决策与
> #1252 / #1253 的边界结论，并修正 §2–§4 中与"两套 UI 运行时"相关的表述。

### 9.1 决策：ECS 是唯一权威

引擎已经有一套完整的 ECS UI：组件定义在 `crates/prism-scene/src/ui.rs`，经
`crates/prism-engine/src/ui/mod.rs:30-33` 再导出，共 11 个反射组件
（`Node` / `Anchors` / `Pivot` / `Margin` / `Layout` / `Style` / `ComputedLayout` /
`Text` / `Canvas` / `Interaction` / `Button`），由
`ui_layout_system` → `ui_input_system` → `ui_render_system` 三个系统驱动
（注册顺序见 `crates/prism-engine/src/engine.rs:796-808`），并通过
`#[derive(Reflect)]` 写进 `.scene.json`。

**结论**：这套 ECS UI 是权威数据模型。`prism-ui` **不得**再拥有一套并行的
`UiTree` / `LayoutStyle` / `UiDrawCommand` / 命中测试；它只保留两层职责：

1. **声明式 builder**：把作者意图装配成 `prism_scene::ui` 组件并 spawn 实体；
2. **交互状态机库**：文本编辑、滚动、拖拽改值、拖放载荷这些 ECS 目前没有的
   行为数据，作为组件/资源挂到 ECS 上，而不是自成一套树。

否决"两套并存叫共享"的理由很直接：两条链已经各自实现了布局求解、命中测试和绘制发射，
语义还不等价（§9.2）。保留两套等于永久维护两个真相。

**当前代码正处于最坏状态**：`editor_system` 在引擎默认调度之后追加
（`projects/editor/crates/editor-app/src/lib.rs:454`），最后一件事是用
`bridge::submit_draw_list` 覆盖引擎刚建好的 `UiDrawList`
（`lib.rs:445`）。也就是说编辑器下 ECS UI 实体被整体抹掉，而 ECS 的
`ui_input_system` 因为没有写 `UiInputState` 而空转。这条覆盖必须删除。

### 9.2 重复面清单（收敛时删除，不迁移）

| 概念 | `prism-ui` | ECS | 处置 |
|---|---|---|---|
| 节点身份 | `NodeId`（`tree.rs:16-19`） | `Entity` + `Node`（`ui.rs:18`） | 删，用 `Entity` |
| 布局样式 | `LayoutStyle` / `Length` / `FlexDirection` / `Align` / `Justify` / `PositionType`（`style.rs:129-187`） | `Style` + `Layout` + `Anchors` + `Pivot` + `Margin`（`ui.rs:215-272`） | 删，用 ECS 组件 |
| 布局求解 | `layout.rs` 全文件（262 行） | `ui_layout_system`（`ui/layout.rs:74-242`） | 删 |
| 交互状态 | `InteractionState`（`style.rs:212-219`） | `Interaction`（`ui.rs:573-581`） | 删，用 ECS 组件 |
| 状态化外观 | `VisualStyle` / `VisualBox` / `VisualPatch`（`style.rs:230-298`） | `Style` + `Button`（`ui.rs:234-272, 591-625`） | 删 |
| 绘制命令 | `UiDrawCommand` / `UiDrawList`（`draw.rs:1-86`） | `UiDrawItem` / `UiQuad` / `UiTextCmd`（`ui/render.rs:14-118`） | 删 |
| 命中与输入路由 | `hit_test` / `process_input`（`tree.rs:339-537`） | `ui_input_system`（`ui/input.rs:59-136`） | 删 |
| 绘制发射 | `emit_node_draw`（`tree.rs:546-644`） | `ui_render_system`（`ui/render.rs:138-237`） | 删 |
| 转换层 | `editor-app/src/bridge.rs`（`bridge.rs:44-161`） | — | 删，改写成"组件写入器" |

### 9.3 ECS 侧必须先补齐的能力

下面每一项都是编辑器跑不起来的硬前提，且都**不是** `prism-ui` 能自己解决的——
它们属于引擎 UI 能力，玩家项目的 UI 同样受益。按依赖顺序排列：

| # | 能力 | 现状与证据 | 补法 |
|---|---|---|---|
| 1 | **字符与 IME 输入** | `PlatformEvent` 只有 `Key { key, pressed }`（`crates/prism-platform/src/event.rs:249,284`），无 Character / Ime / Composition 变体；`UiInputState` 只有光标、左键三态、按键列表（`ui/input.rs:22-34`） | 平台层新增事件变体 → `InputManager` 暴露字符流 → `UiInputState` 携带本帧文本；**这是所有文本编辑功能的地基** |
| 2 | **裁剪区** | `UiDrawItem` 只有 `Quad` / `Text`（`ui/render.rs:73-76`）；GPU scissor 整帧只设一次（`ui_overlay/gpu.rs:779,1081`） | 绘制命令加 clip 区间，或每 quad 带 `clip_rect`；GPU 侧按条目切 scissor |
| 3 | **边框描边** | `Style` 只有 `border_radius`（`ui.rs:265`），无 border width / color | `Style` 增加描边字段；渲染侧用内缩 quad 或 SDF |
| 4 | **线段** | 无 `Line` 命令（`ui/render.rs:73-76`） | 加 `Line` 变体（分隔线、Gizmo 辅助线、动画曲线） |
| 5 | **滚动** | `prism-scene/src/ui.rs` 无任何 scroll 字段；`UiInputState` 不携带滚轮增量 | `Scrollable` 组件（偏移 + 内容尺寸）+ 滚轮路由 |
| 6 | **拖拽载荷** | 只有 `UiPressState { entity }`（`ui/input.rs:40-43`），无载荷类型 | `DragPayload` / `DropTarget` 组件 + 事件 |
| 7 | **虚拟化** | `ui_render_system` 全量遍历所有 `ComputedLayout + Style`（`ui/render.rs:152-192`） | 依赖 #5 的滚动偏移，按可视区间筛选 |
| 8 | **布局 grow / percent / wrap** | `Layout` 明确不做 grow/shrink/权重（`ui.rs:200-204`）；`Style::width/height` 只有 `Option<f32>`（`ui.rs:236-241`） | `Layout` 增加 grow 权重与换行；`Style` 尺寸改为 `Length` 枚举 |
| 9 | **语义稳定 ID** | 兄弟序靠 `SceneEntityId`（`ui/layout.rs:336-342`），无跨帧语义主键 | 见 §9.5 |

**注意 #1 的优先级**：没有字符事件，`text_field` 无论怎么实现都收不到输入。
当前 `bridge.rs:36-37` 把 `text_committed` 恒传空串、`backspace_pressed` 恒传 `false`，
就是这条地基缺失的直接表现。

### 9.4 `prism-ui` 保留什么

| 保留项 | 位置 | 理由 |
|---|---|---|
| 声明式 builder（`ElementBox` + 控件组合矩阵） | `elements.rs:125-330, 375-631` | ECS 手写 spawn 无法替代的作者体验层；改造成直接写入 `prism_scene::ui` 组件 |
| `DraggableValue` 拖拽改值状态机 | `behavior.rs:69-105` | Shift 精细 / Ctrl 吸附 / clamp，ECS 无对应物 |
| `Scrollable::visible_range` / `clamp` | `behavior.rs:116-148` | 虚拟化区间算法，ECS 无 |
| `TextEditable` 状态机 | `behavior.rs:151-199` | 光标 / 选区 / 退格，ECS 无（但与 `prism-editor-core::textinput` 需合并，见 §9.6） |
| `DragPayload` 枚举 | `behavior.rs:45-51` | 跨面板统一载荷（Entity / AssetPath / DockTab） |
| `tween.rs` 插值 | `tween.rs:1-177` | 无 ECS 对应；注意当前是死代码（`animate_scalar` 无调用方） |
| `NodeKey` 语义主键 | `tree.rs:22-92` | 需改成字符串文法，见 §9.5 |

**必须一并修掉的既有缺陷**（否则会原样搬进新架构）：

1. `bridge.rs:151` 丢弃 `Line` / `PushClip` / `PopClip`，导致 `clip_children` 完全失效；
2. `elements.rs:489-504` 的 `text_field` 显示的是 builder 入参，不是 `TextEditable.text`，
   即使输入通了画面也不会变；
3. 虚拟列表用入参 `scroll_offset_y` 算区间（`elements.rs:574-577`），与保留态
   `Scrollable.offset` 从不互相同步；
4. `InteractionState.selected/disabled` 永不置位、`UiAction::TextCommitted` 永不构造、
   `Focusable.tab_index` / `captures_text_keys` 只写不读、`ui-desktop` feature 零引用。

### 9.5 编辑器专用实体与 MCP 节点契约

**编辑器专用实体**（来自 #1254 验收项）：编辑器 UI 实体必须挂一个 `EditorUi` marker，
并被两处排除：

- **存档**：`.scene.json` 写出时跳过。可复用 `DERIVED_COMPONENTS` 的同一套思路
  （`crates/prism-scene/src/json.rs:60-66`）——那里已经把 `ComputedLayout` 定义为
  "运行时输出、作者不写"的派生组件。
- **玩家构建**：`check_deps.py` 保证玩家链不链接编辑器 crate；marker 保证即使实体被
  序列化进场景也不会在玩家侧被当作普通 UI 渲染。

**MCP 节点 id**（来自 #1254 的 `NodeKey` 承诺）：当前**并不成立**。
`prism-ui` 的 `NodeKey` 是 `u64` 哈希（`tree.rs:22-44`），
`inspector.rs:57-69` 把 `field:{entity}:{component}:{field}[{axis}]` 字符串哈希进去后
**丢弃了原串**；`snapshot_debug_nodes`（`tree.rs:646-652`）只返回 `u64`，
没有任何反向映射。而 MCP 侧用的是字符串文法（`editor/crates/prism-editor-egui/src/ui_tree.rs`）：

| 前缀 | 语义 |
|---|---|
| `command:<Name>` | 菜单/快捷键命令 |
| `panel:<tab>` | dock 面板 |
| `doc:<index>` | 打开的文档 |
| `entity:<id>` | 场景实体 |
| `asset:<path>` | 资产条目 |
| `field:<entity>:<component>:<path>` | 检查器字段 |
| `view:<tab>:entity:<id>` | 视口内物体 |
| `dialog:<kind>[:<button>]` | 弹窗 |

**要求**：`NodeKey` 改为字符串文法本身（或保留字符串 + 稳定哈希两条视图），
文法定义放进 `prism-editor-core`（UI 框架无关、`crates/prism-editor-mcp` 已可依赖），
成为 MCP、builder、跨帧状态保留的**单一事实源**。不要维护两套 id。

### 9.6 与 `prism-editor-core` 的分工

`prism-editor-core` 确认是 UI 框架无关的（`Cargo.toml:8-16` 只有
`serde` / `serde_json` / `log` / `image` / `glam` / `prism-ini`，全目录
grep `egui::` 零命中）。因此 §1 表格里"绑定 `prism-editor-core`"是成立的。

两处需要合并的重复：

- **文本模型**：`prism-editor-core::textinput`（`textinput.rs:12-206`，支持选区、
  按词移动、Unicode 安全）与 `prism-ui::TextEditable`（`behavior.rs:151-199`，
  只有 insert/backspace）是两套。前者更完整，且**当前无人使用**——egui 编辑器一律用
  `egui::TextEdit`。收敛时以 core 的为准，`prism-ui` 那套删除。
- **面板 id 注册表**：`ui_tree::panel_title` 硬编码英文表，与
  `Workspace::panel_title`（i18n）不一致，导致 MCP snapshot 里部分 tab 的 label
  退化成 `"Panel"`。收敛时以 core 的为准。

### 9.7 工程边界补充（来自 #1252 / #1253 / #1254）

1. **CI 覆盖**：`projects/editor` 目前只有 `scripts/pr_ready.py:401-402` 的
   `Cargo.lock` 一致性检查，**不在** `scripts/check_features.py` 的 feature 矩阵里。
   `prism-ui` 的单元测试与 `cargo check` 需要进本地门禁。
2. **项目边界**：编辑器仍硬编码 `projects/template` / `template.exe`
   （`editor/crates/prism-editor-core/src/build.rs`），项目根靠 cwd 向上找
   `assets/` 发现（`editor/crates/prism-editor-egui/src/app.rs:218`）。
   自举编辑器落地时**必须**改为显式项目路径参数，不要复制这套假设。
3. **组合根**：`prism-app` 保持库形态、不加 `main`；`projects/editor` 作为独立
   workspace 的 composition root，与 `projects/template` 同构（#1252）。

### 9.8 修订后的分期

§7 的分期在 ECS 权威下需要重排：

| 阶段 | 内容 | 前置 |
|---|---|---|
| **Phase 0** | 平台字符/IME 事件 + `UiInputState` 文本流 | 无 |
| **Phase 1** | ECS UI 能力补齐：clip、border、Line、Scrollable、DragPayload、虚拟化、grow/percent | Phase 0（滚动与文本编辑依赖输入） |
| **Phase 2** | `prism-ui` 降级：删除 §9.2 的重复面，`bridge.rs` 改为组件写入器，删除 `lib.rs:445` 的 draw list 覆盖 | Phase 1 |
| **Phase 3** | 面板迁移：以 `prism-editor-core` 的 dock/undo/i18n 为数据源，逐个面板搬离 egui | Phase 2 |
| **Phase 4** | 多窗口（`prism-platform` + 同一 `VkDevice` 多 swapchain） | Phase 3 |

Phase 0–1 属于**引擎能力**，玩家项目同样受益，应先于编辑器迁移落地；
Phase 2 之后 `projects/editor` 才具备替换 `editor/` 的条件。

---

## 10. 参考

- [`project-module-architecture.md`](project-module-architecture.md) — 宿主边界与 preview 模块 ABI
- [`editor-egui.md`](editor-egui.md) — 现役 egui 编辑器（迁移来源）
- [`editor-mcp.md`](editor-mcp.md) — MCP 自动化契约
- [`REFLECT.md`](REFLECT.md) — 组件反射与 RTTIBin
- [`scripting-layer.md`](scripting-layer.md) — 脚本层同样以 ECS + `HostApi` 为边界
- issue #1252 / #1253 / #1254 — 组合根、SDK 边界、UI 权威归属

