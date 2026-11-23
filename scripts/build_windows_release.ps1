#Requires -Version 5.1
<#
.SYNOPSIS
    LanVault Windows 8C16T 极限性能发行版一键构建脚本。
.DESCRIPTION
    以 16 并发线程编译 Rust 原生 FFI 动态库 (lanvault_ffi.dll) 与可选 Windows SCM 服务
    (lanvault-win-svc.exe)，随后构建混淆且剥离调试符号的 Flutter Windows Release 产物，
    最终统一归档至 dist/windows-release/。
.PARAMETER SkipWinSvc
    跳过 lanvault-win-svc.exe 的编译与归档（仅打包桌面主程序与 lanvault_ffi.dll）。
.PARAMETER Clean
    构建前清理 dist/windows-release/ 目录。
#>
[CmdletBinding()]
param(
    [switch]$SkipWinSvc,
    [switch]$Clean
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# 1. 锁定 8 核 16 线程并行构建环境变量
$env:CARGO_BUILD_JOBS = "16"
$env:CMAKE_BUILD_PARALLEL_LEVEL = "16"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$FlutterDir = Join-Path $RepoRoot "flutter_app"
$DistDir = Join-Path $RepoRoot "dist\windows-release"
$TargetTriple = "x86_64-pc-windows-msvc"

function Assert-Command {
    param([Parameter(Mandatory = $true)][string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "未在 PATH 中找到必需工具 '$Name'，请先安装并配置环境变量。"
    }
}

Write-Host "[LanVault][Windows] 校验构建工具链..." -ForegroundColor Cyan
Assert-Command "cargo"
Assert-Command "flutter"

if ($Clean -and (Test-Path $DistDir)) {
    Write-Host "[LanVault][Windows] 清理历史归档目录: $DistDir" -ForegroundColor Yellow
    Remove-Item -Path $DistDir -Recurse -Force
}

# 2. 编译 Rust 原生库（lanvault-ffi + 可选 lanvault-win-svc）
Push-Location $RepoRoot
try {
    $CargoArgs = @(
        "build",
        "--release",
        "-p", "lanvault-ffi"
    )
    if (-not $SkipWinSvc) {
        $CargoArgs += @("-p", "lanvault-win-svc")
    }
    $CargoArgs += @("--target", $TargetTriple, "-j", "16")

    Write-Host "[LanVault][Windows] 执行: cargo $($CargoArgs -join ' ')" -ForegroundColor Cyan
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo Release 构建失败，退出码: $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$CargoReleaseDir = Join-Path $RepoRoot "target\$TargetTriple\release"
$FfiDllPath = Join-Path $CargoReleaseDir "lanvault_ffi.dll"
if (-not (Test-Path $FfiDllPath)) {
    throw "未找到编译产物: $FfiDllPath"
}

$WinSvcExePath = Join-Path $CargoReleaseDir "lanvault-win-svc.exe"
if (-not $SkipWinSvc -and -not (Test-Path $WinSvcExePath)) {
    throw "未找到 Windows SCM 服务编译产物: $WinSvcExePath"
}

# 3. 构建 Flutter Windows Release（开启 Dart 混淆与符号剥离）
Push-Location $FlutterDir
try {
    $SymbolsDir = "build/symbols/windows"
    $FlutterArgs = @(
        "build", "windows",
        "--release",
        "--obfuscate",
        "--split-debug-info=$SymbolsDir"
    )
    Write-Host "[LanVault][Windows] 执行: flutter $($FlutterArgs -join ' ')" -ForegroundColor Cyan
    & flutter @FlutterArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Flutter Windows Release 构建失败，退出码: $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

# 4. 统一归档至 dist/windows-release/
$FlutterReleaseDir = Join-Path $FlutterDir "build\windows\x64\runner\Release"
if (-not (Test-Path $FlutterReleaseDir)) {
    # 兼容旧版 Flutter 输出路径
    $LegacyReleaseDir = Join-Path $FlutterDir "build\windows\runner\Release"
    if (Test-Path $LegacyReleaseDir) {
        $FlutterReleaseDir = $LegacyReleaseDir
    }
    else {
        throw "未找到 Flutter Windows Release 输出目录: $FlutterReleaseDir"
    }
}

New-Item -ItemType Directory -Path $DistDir -Force | Out-Null
Write-Host "[LanVault][Windows] 归档发行版产物至: $DistDir" -ForegroundColor Cyan

Copy-Item -Path (Join-Path $FlutterReleaseDir "*") -Destination $DistDir -Recurse -Force
Copy-Item -Path $FfiDllPath -Destination (Join-Path $DistDir "lanvault_ffi.dll") -Force

if (-not $SkipWinSvc -and (Test-Path $WinSvcExePath)) {
    Copy-Item -Path $WinSvcExePath -Destination (Join-Path $DistDir "lanvault-win-svc.exe") -Force
}

Write-Host ""
Write-Host "[LanVault][Windows] 发行版构建完成，归档清单:" -ForegroundColor Green
Get-ChildItem -Path $DistDir -File | Format-Table Name, @{Label = "Size(MB)"; Expression = { [math]::Round($_.Length / 1MB, 2) } }, LastWriteTime -AutoSize
Write-Host "调试符号已剥离至: $(Join-Path $FlutterDir 'build\symbols\windows')" -ForegroundColor DarkGray
