# 发布档构建（Windows）：nightly `-Z build-std` + immediate-abort。
# 与 `scripts/build-dist.sh` 等价，parameter /理由见那边的注释。
#
#   powershell -File scripts/build-dist.ps1
#   powershell -File scripts/build-dist.ps1 projects/template
#   powershell -File scripts/build-dist.ps1 projects/template x86_64-pc-windows-gnu
#   powershell -File scripts/build-dist.ps1 -Size projects/template    # 极限volume （tiny + 禁内联）
#   powershell -File scripts/build-dist.ps1 -Pack projects/template    # again 套 UPX compress 壳
#
# -Size correct  `.sh --size`：切 `tiny` profile（opt-level="z"）并禁内联，比 dist again small  ~30%，
# 代价is  CPU 热path slow  10–30%。-Size 与 -Profile same 时给时以 -Profile 为准。
# -Pack 只给publish 档用，压完must 做代码签名（not 签名的自decompress 壳will 被 Windows Defender 等误报）。
# 找not 到 upx 时只hint 、not failure 。
param(
    [string]$Project = "projects/template",
    [string]$Target = "",
    [string]$Profile = "dist",
    [string]$Nightly = "nightly",
    [switch]$Pack,
    [switch]$Size
)

$ErrorActionPreference = "Stop"
# scripts/ 的on 一级就is 仓库根；parameter 一律用position 传，PowerShell 5.1 的 Join-Path not 吃 -LiteralPath。
$Root = Split-Path -Parent $PSScriptRoot
$Manifest = Join-Path $Root (Join-Path $Project "Cargo.toml")
if (-not (Test-Path $Manifest)) { throw "找不到 $Manifest" }

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw "需要 rustup（用它装 nightly + rust-src）"
}

$installed = rustup toolchain list
if ($installed -notmatch "^$Nightly") {
    Write-Host "build-dist: 安装 $Nightly（首次约 100 MB）"
    rustup toolchain install $Nightly --profile minimal
}
$components = rustup component list --toolchain $Nightly
if ($components -notmatch "rust-src \(installed\)") {
    Write-Host "build-dist: 安装 rust-src"
    rustup component add rust-src --toolchain $Nightly
}

if ([string]::IsNullOrEmpty($Target)) {
    $Target = ((rustc -vV | Select-String '^host: ').ToString() -replace '^host: ', '').Trim()
}

$name = (Select-String -Path $Manifest -Pattern '^name = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value

# -Size correct  `.sh --size`：default 档bit 切 tiny，again 叠「禁内联」这刀（纯volume 手段）。
# 显式给了 -Profile 就尊heavy 它，只加禁内联。
$ExtraRustFlags = ""
if ($Size) {
    if ($Profile -eq "dist") { $Profile = "tiny" }
    $ExtraRustFlags = " -C llvm-args=-inline-threshold=0 -C llvm-args=-inlinehint-threshold=0"
}

Write-Host "build-dist: $Project ($name) → $Target / profile=$Profile"

# parameter 只走命令行，not 写进 Cargo.toml：stable build not 受影响。
$env:RUSTFLAGS = "-Zunstable-options -Cpanic=immediate-abort$ExtraRustFlags"
cargo "+$Nightly" build `
    --manifest-path $Manifest `
    --profile $Profile `
    --package $name `
    --target $Target `
    -Z build-std=std,panic_abort
if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（exit $LASTEXITCODE）" }

$bin = Join-Path $Root (Join-Path $Project "target\${Target}\${Profile}\${name}.exe")
if (Test-Path $bin) {
    # note 别用 `$size`：PowerShell variable size 写not 敏感，will 和 `-Size` switch 撞名。
    $bytes = (Get-Item $bin).Length
    Write-Host ("build-dist: {0}  {1} bytes ({2:N2} MB)" -f $bin, $bytes, ($bytes / 1MB))
} else {
    Write-Host "build-dist: 构建成功但没找到 $bin（检查包名与 bin 名是否一致）"
}

# optional compress 壳（-Pack）。compress not is build 的positive 确性before 提，any 一步failure 都只hint 。
if ($Pack) {
    if (-not (Test-Path $bin)) {
        Write-Host "build-dist: -Pack 跳过（没有找到产物）"
    } elseif ($Target -match 'android') {
        Write-Host "build-dist: -Pack 跳过：Android 产物不打压缩壳"
    } elseif (-not (Get-Command upx -ErrorAction SilentlyContinue)) {
        Write-Host "build-dist: 未找到 upx，跳过压缩（-Pack 是可选项，构建照常成功）"
        Write-Host "  安装：winget install upx，或从 https://upx.github.io 下载后加进 PATH"
        Write-Host "  未压缩产物已保留：$bin"
    } else {
        $before = (Get-Item $bin).Length
        Write-Host "build-dist: upx --best --lzma"
        & upx --best --lzma $bin
        if ($LASTEXITCODE -eq 0) {
            $after = (Get-Item $bin).Length
            $ratio = if ($before -gt 0) { (1 - $after / $before) * 100 } else { 0 }
            Write-Host ("build-dist: packed  {0} -> {1} bytes ({2:N2} MB, 压缩率 {3:N1}%)" -f $before, $after, ($after / 1MB), $ratio)
        } else {
            Write-Host "build-dist: upx 压缩失败，保留未压缩产物（构建本身仍然成功）"
        }
    }
    Write-Host "build-dist: 注意：压缩后请对产物做代码签名，否则 Windows Defender 等会误报（未签名的自解压壳是典型启发式特征）"
}
