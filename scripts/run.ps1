# Prisma 一键构建脚本 (Windows)
# 用法: in  PowerShell in  cd 到本directory after execute   .\run.ps1
# need : already install  Vulkan SDK (default  D:\VulkanSDK\1.4.350.0，自带 slangc) 和 Rust
$ErrorActionPreference = "Stop"

$sdk = "D:\VulkanSDK\1.4.350.0"
if (-not (Test-Path "$sdk\Bin\glslc.exe")) {
    # 回退到常见install path
    $sdk = "C:\VulkanSDK\1.4.350.0"
}
if (-not (Test-Path "$sdk\Bin\glslc.exe")) {
    Write-Warning "未找到 Vulkan SDK，请从 https://vulkan.lunarg.com/sdk/home 安装，并确认路径。"
}

$env:PATH = "$env:USERPROFILE\.cargo\bin;$sdk\Bin;" + $env:PATH
$env:VK_SDK = $sdk
$env:VK_LAYER_PATH = "$sdk\Bin"
$env:VULKAN_SDK = $sdk
$env:RUST_LOG = "warn,tracy_client=off"

# 脚本bit 于 scripts/ below ，切到仓库根directory
Set-Location (Split-Path -Parent $PSScriptRoot)

# heavy new compile  Slang shader  (slangc 保留 vertexMain/fragmentMain 入口名)。
# slangc 随 Vulkan SDK 提供；.spv not 提交，must compile after 引擎才使用most new shader 。
if (Get-Command slangc -ErrorAction SilentlyContinue) {
    Write-Host "重新编译 Slang 着色器..."
    & bash assets/shaders/compile.sh
    if ($LASTEXITCODE -ne 0) { Write-Error "Slang 着色器编译失败"; exit 1 }
} else {
    Write-Warning "未找到 slangc。请确认 Vulkan SDK 已含 slangc，或单独安装 Slang。"
    Write-Warning "跳过着色器编译 (将使用上次编译产物或 CI 产出的 .spv)。"
}

# build 整个 workspace（render  / ECS / 引擎 / 资产管line 均以库形式build ）。
# note ：桌face 可execute 入口already 迁移到 launcher/（Tauri independent  workspace），
# 根 workspace 没have 可run 的 bin，`cargo run` not again 适用。
Write-Host "构建 Prisma (debug)..."
cargo build
if ($LASTEXITCODE -ne 0) { Write-Error "构建失败"; exit 1 }

# generate run 时scene 包：scenes.json -> cooked RSCN -> assets/scenes.pak。
# 游戏run 时只从该 .pak 的path 清单parse scene ，therefore open 发start 动before 也must generate 。
# each 工程的 manifest in 自己的 assets/ below ，仓库根只放引擎input （shaders、GI 探针）。
$SceneProject = "projects/template"
if (Test-Path "$SceneProject/assets/scenes.json") {
    Write-Host "构建场景包 ($SceneProject/assets/scenes.pak)..."
    Push-Location $SceneProject
    try {
        cargo run -q --manifest-path (Join-Path $PWD "../../Cargo.toml") -p prism-build-pipeline -- build-scenes
        if ($LASTEXITCODE -ne 0) { Write-Error "场景包构建失败"; exit 1 }
    } finally { Pop-Location }
}

Write-Host ""
Write-Host "构建完成。启动入口："
Write-Host "  launcher/ (Tauri 桌面壳 + Android APK)：cd launcher && pnpm tauri dev"
Write-Host "  Android：scripts/build-android.ps1 (Rust cdylib) + launcher/build_release.sh (APK)"
