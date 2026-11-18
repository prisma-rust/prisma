# 番外篇：在 Android chroot 容器中直通 Adreno GPU（Turnip + KGSL）

> **层级**：L4 平台接入与构建运维（[`build-guide-termux.md`](build-guide-termux.md) 番外篇）
> **适用环境**：已 Root 的 Android `aarch64` `chroot` 容器（高通 Adreno GPU，容器内可访问 `/dev/kgsl-3d0`）

在 [`build-guide-termux.md`](build-guide-termux.md) 中，手机端通常只负责编译与静态检查，遇到 Vulkan GPU 测试会因为没有硬件驱动而跳过或触发段错误。但如果手机处于 **chroot** 环境且芯片为高通骁龙（Adreno），其实可以直接在容器内跑起**原生硬件 Vulkan**，甚至直接在手机端运行 `prism-render` 的离屏 GPU 集成测试。

---

## 一、为什么默认装 `mesa-vulkan-drivers` 不行？

1. 容器内虽然已经映射了高通内核图形设备节点 `/dev/kgsl-3d0`（权限 `crw-rw-rw-`），但 Ubuntu 官方源打包的 `mesa-vulkan-drivers` 默认只编译了 Linux DRM/KMS 后端（查找 `/dev/dri/renderD*`）。
2. 在 Android 内核下没有 DRM 节点，`vulkan-loader` 会跳过 `freedreno_icd.json` 并退回到 `lvp_icd.json`（llvmpipe 软件渲染），导致性能极差或在容器内直接崩溃。
3. 而在 `chroot` 中没有 `proot` 的 `ptrace` 拦截开销，只要编译一份开启 **KGSL 后端**（`-Dfreedreno-kmds=kgsl`）的 Mesa Turnip 驱动，用户态即可通过 `ioctl` 直接驱动 Adreno GPU。

---

## 二、编译纯离屏 Turnip + KGSL 驱动

如果只跑引擎的离屏 Vulkan 测试（`GraphRenderer::offscreen_new` / Compute Pass），不需要安装 X11、Wayland 或 LLVM，只需编译纯 Vulkan ICD：

### 1. 安装最小编译依赖

```bash
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y \
  vulkan-tools meson ninja-build glslang-tools \
  python3-mako python3-yaml bison flex libdrm-dev zlib1g-dev libexpat1-dev
```

### 2. 浅克隆 Mesa 并编译安装

> **注意**：许多容器的 `/tmp` 是较小的 `tmpfs`（如 256 MB），切勿把 Mesa 源码放在 `/tmp`，建议放在磁盘空间充足的 `~/.cache/` 下，编完即删。

```bash
git clone --depth 1 https://gitlab.freedesktop.org/mesa/mesa.git ~/.cache/mesa-turnip
cd ~/.cache/mesa-turnip

meson setup build \
  -Dvulkan-drivers=freedreno \
  -Dfreedreno-kmds=kgsl \
  -Dgallium-drivers= \
  -Dplatforms= \
  -Dglx=disabled \
  -Dllvm=disabled \
  -Dshared-llvm=disabled \
  -Dbuildtype=release \
  -Dprefix=/usr/local

ninja -C build -j"$(nproc)"
ninja -C build install
rm -rf ~/.cache/mesa-turnip
```

编译产物会安装到：
- 驱动库：`/usr/local/lib/aarch64-linux-gnu/libvulkan_freedreno.so`
- ICD 清单：`/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json`

---

## 三、启用与实测验证

通过 `VK_ICD_FILENAMES` 显式锁定刚编译的 KGSL ICD（避免加载器回退到系统的 llvmpipe）：

```bash
export VK_ICD_FILENAMES=/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json
vulkaninfo --summary
```

在实机（Ubuntu 26.04 aarch64 chroot + Adreno 830）上的验证输出：
- `deviceName = Adreno (TM) 830v1`
- `driverID = DRIVER_ID_MESA_TURNIP`
- `apiVersion = 1.4.368`

随后可直接运行引擎的 GPU 集成测试：

```bash
# SHaRC 缓存生命周期 GPU 测试（5/5 通过）
VK_ICD_FILENAMES=/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json \
  cargo test -p prism-render --test sharc_lifecycle_gpu -- --nocapture

# PRT64 + 64 叉树管线 GPU 测试（1/1 通过）
VK_ICD_FILENAMES=/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json \
  cargo test -p prism-render --test prt_gpu -- --nocapture
```

---

## 四、边界与注意事项

- **仅限高通 Adreno**：Turnip + KGSL 依赖 `/dev/kgsl-3d0`。若设备为 ARM Mali / 天玑 / Exynos，需改用 `virglrenderer-android` + ANGLE 或 Vortek socket 转发。
- **仅含离屏能力**：上述配置设了 `-Dplatforms=`，不带 X11/Wayland WSI 扩展，专用于无窗口离屏渲染与自动化测试；若需桌面窗口显示，需搭配 `Termux-X11` 并开启 `-Dplatforms=x11`。
