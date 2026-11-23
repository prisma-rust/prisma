# Prisma 开发启动器（RustRover / 终端通用）
#
#   .\scripts\dev.ps1                # compile shader (have  slangc 时) -> heavy 打scene 包 -> run 游戏
#   .\scripts\dev.ps1 -Target editor # start 动 Tauri 编辑器 (pnpm -C editor tauri dev)
#
# 常用switch ：
#   -SkipShaders   not heavy 编 Slang（只改了 .rs 时省几seconds ）
#   -SkipScenes    not heavy 打 projects/template/assets/scenes.pak（没改scene /预制体时available ）
#   -Release       游戏用 --release build
#
# description ：根 workspace 没have 可run 的 bin，游戏二进制in  projects/template（independent  workspace），
# 它按 cwd 及其父directory 找 `assets/scenes.pak` / `assets/settings.ini`；工程的资产in 自己
# 的 assets/ below ，so scene 包要in  projects/template inside build 。
param(
    [ValidateSet("game", "editor")]
    [string]$Target = "game",
    [switch]$SkipShaders,
    [switch]$SkipScenes,
    [switch]$Release
)

$ErrorActionPreference = "Stop"

# 脚本in  scripts/ below ，切到仓库根directory （游戏按 cwd parse  assets/）。
Set-Location (Split-Path -Parent $PSScriptRoot)

function Resolve-VulkanSdk {
    $candidates = @(
        $env:VULKAN_SDK,
        "D:\VulkanSDK\1.4.350.0",
        "C:\VulkanSDK\1.4.350.0"
    ) | Where-Object { $_ -and (Test-Path (Join-Path $_ "Bin\glslc.exe")) }
    return $candidates | Select-Object -First 1
}

$sdk = Resolve-VulkanSdk
if ($sdk) {
    $env:VK_SDK = $sdk
    $env:VULKAN_SDK = $sdk
    $env:VK_LAYER_PATH = Join-Path $sdk "Bin"
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$(Join-Path $sdk 'Bin');$env:PATH"
} else {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

if (-not $env:RUST_LOG) { $env:RUST_LOG = "warn,tracy_client=off" }

if ($Target -eq "editor") {
    Write-Host "启动编辑器 (Tauri)..." -ForegroundColor Cyan
    pnpm -C editor tauri dev
    exit $LASTEXITCODE
}

if (-not $SkipShaders) {
    if (Get-Command slangc -ErrorAction SilentlyContinue) {
        Write-Host "编译 Slang 着色器..." -ForegroundColor Cyan
        & bash assets/shaders/compile.sh
        if ($LASTEXITCODE -ne 0) { Write-Error "着色器编译失败"; exit 1 }
    } else {
        Write-Warning "未找到 slangc，跳过着色器编译（沿用上次产物/CI 的 .spv）。"
    }
}

if (-not $SkipScenes) {
    # scene 清单in each 工程自己的 assets/ below ，仓库根只放引擎input 。
    Write-Host "重打场景包 (projects/template/assets/scenes.pak)..." -ForegroundColor Cyan
    Push-Location "projects/template"
    try {
        cargo run -q --manifest-path "../../Cargo.toml" -p prism-build-pipeline -- build-scenes
        if ($LASTEXITCODE -ne 0) { Write-Error "场景包构建失败"; exit 1 }
    } finally { Pop-Location }
}

$cargoArgs = @("run", "--manifest-path", "projects/template/Cargo.toml", "--bin", "template")
if ($Release) { $cargoArgs += "--release" }

Write-Host "运行游戏: cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
cargo @cargoArgs
exit $LASTEXITCODE
