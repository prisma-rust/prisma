# Emergence（涌现）

`Emergence` 是基于 Prisma 引擎搭建的 **v3.0 硬核沙盒标签与物理推演项目**（独立 workspace，位于 `projects/emergence/`）。

- 完整引擎演进规划与架构映射请见 [`DESIGN.md`](DESIGN.md)。
- v3.0 五维标签（基础环境状态枚举、固有材质位、瞬态危化位、空间拓扑位、外部刺激与动态时钟）的 16 字节紧凑内存内核实现在 [`src/tags.rs`](src/tags.rs)。

## 目录结构

```text
projects/emergence/
  Cargo.toml              # 独立 workspace（player 与 preview-module 双形态）
  DESIGN.md               # 围绕 Prisma 引擎的 v3.0 沙盒开发计划
  README.md               # 本说明文档
  assets/
    settings.ini          # 窗口与 320x180 像素渲染配置
    scenes.json           # 场景清单（lab）
    scenes/
      lab.scene.json      # 初始实验室正交相机场景（v3）
  src/
    main.rs               # 桌面端可执行入口
    lib.rs                # 玩家 build_app / Android JNI / 编辑器 preview-module 入口
    tags.rs               # v3.0 全景标签 16 字节 PhysicalCell 内核与单元测试
```

## 构建与校验

检查并运行 `tags` 内核单元测试（编辑器 `preview-module` 形态，不链接渲染后端，适用于全平台快速验证）：

```bash
cargo test --manifest-path projects/emergence/Cargo.toml --no-default-features --features preview-module
```

构建桌面玩家二进制（需 Vulkan / 音频系统库与已编译着色器）：

```bash
cargo run -p prism-build-pipeline -- build-scenes --project projects/emergence
cargo build --manifest-path projects/emergence/Cargo.toml
```
