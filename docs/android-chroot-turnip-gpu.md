# Sidebar: Directly pass through Adreno GPU (Turnip + KGSL) in Android chroot container

> **Tier**: L4 platform integration and build ops ([`build-guide-termux.md`](build-guide-termux.md) sidebar)
> **Applicable environment**: Rooted Android `aarch64` `chroot` container (Qualcomm Adreno GPU, container can access `/dev/kgsl-3d0`)

In [`build-guide-termux.md`](build-guide-termux.md), mobile usually only handles compilation and static checks; Vulkan GPU tests are skipped or trigger segfaults due to no hardware driver. But if the phone is in a **chroot** environment and the chip is Qualcomm Snapdragon (Adreno), you can actually run **native hardware Vulkan** inside the container, and even run `prism-render`'s offscreen GPU integration tests directly on the phone.

---

## 1. Why installing `mesa-vulkan-drivers` by default doesn't work

1. Although the Qualcomm kernel graphics device node `/dev/kgsl-3d0` (permission `crw-rw-rw-`) is mapped inside the container, the `mesa-vulkan-drivers` packaged from Ubuntu official source defaults to compiling only the Linux DRM/KMS backend (looking for `/dev/dri/renderD*`).
2. Under Android kernel, there is no DRM node, so `vulkan-loader` skips `freedreno_icd.json` and falls back to `lvp_icd.json` (llvmpipe software rendering), resulting in extremely poor performance or direct crash inside the container.
3. And in `chroot` there is no `proot`'s `ptrace` interception overhead; as long as a Mesa Turnip driver with **KGSL backend** enabled (`-Dfreedreno-kmds=kgsl`) is compiled, user space can directly drive Adreno GPU via `ioctl`.

---

## 2. Compile pure offscreen Turnip + KGSL driver

If only running the engine's offscreen Vulkan tests (`GraphRenderer::offscreen_new` / Compute Pass), no need to install X11, Wayland, or LLVM, just compile pure Vulkan ICD:

### 1. Install minimal build dependencies

```bash
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y \
  vulkan-tools meson ninja-build glslang-tools \
  python3-mako python3-yaml bison flex libdrm-dev zlib1g-dev libexpat1-dev
```

### 2. Shallow-clone Mesa and compile/install

> **Note**: Many containers' `/tmp` is a small `tmpfs` (e.g. 256 MB), do not put Mesa source code in `/tmp`; suggest placing under `~/.cache/` with sufficient disk space, delete after compiling.

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

Build artifacts installed to:
- Driver library: `/usr/local/lib/aarch64-linux-gnu/libvulkan_freedreno.so`
- ICD manifest: `/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json`

---

## 3. Enable and verify

Explicitly lock to the newly compiled KGSL ICD via `VK_ICD_FILENAMES` (avoid loader falling back to system llvmpipe):

```bash
export VK_ICD_FILENAMES=/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json
vulkaninfo --summary
```

Verification output on real device (Ubuntu 26.04 aarch64 chroot + Adreno 830):
- `deviceName = Adreno (TM) 830v1`
- `driverID = DRIVER_ID_MESA_TURNIP`
- `apiVersion = 1.4.368`

Then directly run the engine's GPU integration tests:

```bash
# SHaRC cache lifecycle GPU test (5/5 passed)
VK_ICD_FILENAMES=/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json \
  cargo test -p prism-render --test sharc_lifecycle_gpu -- --nocapture

# PRT64 + 64-ary tree pipeline GPU test (1/1 passed)
VK_ICD_FILENAMES=/usr/local/share/vulkan/icd.d/freedreno_icd.aarch64.json \
  cargo test -p prism-render --test prt_gpu -- --nocapture
```

---

## 4. Boundaries and notes

- **Qualcomm Adreno only**: Turnip + KGSL depends on `/dev/kgsl-3d0`. If the device is ARM Mali / Dimensity / Exynos, need to switch to `virglrenderer-android` + ANGLE or Vortek socket forwarding.
- **Offscreen only**: The above configuration sets `-Dplatforms=`, no X11/Wayland WSI extensions, dedicated to windowless offscreen rendering and automation testing; if desktop window display is needed, pair with `Termux-X11` and enable `-Dplatforms=x11`.
