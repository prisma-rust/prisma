# Prisma 自举式编辑器 UI 架构设计 Spec

> **目标**：在不破坏 [AGENTS.md](../AGENTS.md) 依赖边界的前提下，基于 Prisma 现有的 `prism-ecs`、`prism-reflect`、`prism-render`（`ui_overlay` / 离屏 `VkImage`）与 `prism-editor-core`，设计一套**无继承、无 `Rc<RefCell<T>>`、高度组合复用**的保留模式 UI 内核与编辑器架构。

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
