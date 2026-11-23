#Requires -Version 5.1
<#
.SYNOPSIS
    EveryoneNAS Android (arm64-v8a) 8C16T 极限性能发行版一键构建脚本。
.DESCRIPTION
    以 16 并发线程通过 cargo-ndk 编译 ARM64 NEON/AES/SHA2/CRC 优化且满足 Android 15 16KB 页对齐的 liblanvault_ffi.so，
    随后执行 R8 Full Mode + Dart 混淆与符号剥离的 Flutter Android Release 构建，
    并将最终 APK（及可选 AAB）归档至 dist/android-release/。
.PARAMETER BuildAab
    在生成 split-per-abi APK 的同时额外构建 Play Store / 分发用 App Bundle (.aab)。
.PARAMETER Clean
    构建前清理 dist/android-release/ 目录。
#>
[CmdletBinding()]
param(
    [switch]$BuildAab,
    [switch]$Clean
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$AppVersion = "0.2.0"

# 1. 锁定 8 核 16 线程并发、ARM64 NEON/Crypto/CRC 指令集优化与 Android 15 16KB 页对齐链接标志
$env:CARGO_BUILD_JOBS = "16"

$Arm64FeatureFlag = "-C target-feature=+neon,+aes,+sha2,+crc"
$Arm64PageSizeFlag = "-C link-arg=-Wl,-z,max-page-size=16384"
if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS)) {
    $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS = "$Arm64FeatureFlag $Arm64PageSizeFlag"
}
else {
    if ($env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS -notlike "*+crc*") {
        $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS = "$($env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS) $Arm64FeatureFlag"
    }
    if ($env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS -notlike "*max-page-size=16384*") {
        $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS = "$($env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS) $Arm64PageSizeFlag"
    }
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$FlutterDir = Join-Path $RepoRoot "flutter_app"
$JniLibsDir = Join-Path $FlutterDir "android\app\src\main\jniLibs"
$DistDir = Join-Path $RepoRoot "dist\android-release"

function Assert-Command {
    param([Parameter(Mandatory = $true)][string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "未在 PATH 中找到必需工具 '$Name'，请先安装并配置环境变量。"
    }
}

Write-Host "[EveryoneNAS][Android] 校验构建工具链..." -ForegroundColor Cyan
Assert-Command "cargo"
Assert-Command "flutter"

$KeyPropertiesPath = Join-Path $FlutterDir "android\key.properties"
if (-not (Test-Path -LiteralPath $KeyPropertiesPath)) {
    throw "Release build requires android/key.properties so the APK is not debug-signed."
}

& cargo ndk --version | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "未检测到 cargo-ndk，请先执行: cargo install cargo-ndk"
}

if ($Clean -and (Test-Path $DistDir)) {
    Write-Host "[EveryoneNAS][Android] 清理历史归档目录: $DistDir" -ForegroundColor Yellow
    Remove-Item -Path $DistDir -Recurse -Force
}

# 2. 通过 cargo-ndk 编译 arm64-v8a 原生 .so
Push-Location $RepoRoot
try {
    $CargoNdkArgs = @(
        "ndk",
        "-t", "arm64-v8a",
        "-o", "flutter_app/android/app/src/main/jniLibs",
        "build",
        "--release",
        "-p", "lanvault-ffi",
        "-j", "16"
    )
    Write-Host "[EveryoneNAS][Android] 执行: cargo $($CargoNdkArgs -join ' ')" -ForegroundColor Cyan
    Write-Host "[EveryoneNAS][Android] RUSTFLAGS (aarch64-linux-android): $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS" -ForegroundColor DarkCyan
    & cargo @CargoNdkArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo-ndk 编译 arm64-v8a 失败，退出码: $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$SoPath = Join-Path $JniLibsDir "arm64-v8a\liblanvault_ffi.so"
if (-not (Test-Path $SoPath)) {
    throw "未找到原生共享库产物: $SoPath"
}

# 3. 构建 Flutter Android Release APK（R8 Full Mode + Dart 混淆 + 符号剥离）
Push-Location $FlutterDir
try {
    $SymbolsDir = "build/symbols/android"
    $ApkArgs = @(
        "build", "apk",
        "--release",
        "--target-platform", "android-arm64",
        "--split-per-abi",
        "--obfuscate",
        "--split-debug-info=$SymbolsDir"
    )
    Write-Host "[EveryoneNAS][Android] 执行: flutter $($ApkArgs -join ' ')" -ForegroundColor Cyan
    & flutter @ApkArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Flutter Android APK Release 构建失败，退出码: $LASTEXITCODE"
    }

    if ($BuildAab) {
        $AabArgs = @(
            "build", "appbundle",
            "--release",
            "--target-platform", "android-arm64",
            "--obfuscate",
            "--split-debug-info=$SymbolsDir"
        )
        Write-Host "[EveryoneNAS][Android] 执行: flutter $($AabArgs -join ' ')" -ForegroundColor Cyan
        & flutter @AabArgs
        if ($LASTEXITCODE -ne 0) {
            throw "Flutter Android AAB Release 构建失败，退出码: $LASTEXITCODE"
        }
    }
}
finally {
    Pop-Location
}

# 4. 归档至 dist/android-release/
New-Item -ItemType Directory -Path $DistDir -Force | Out-Null

$ApkOutDir = Join-Path $FlutterDir "build\app\outputs\flutter-apk"
$Arm64Apk = Join-Path $ApkOutDir "app-arm64-v8a-release.apk"
if (-not (Test-Path $Arm64Apk)) {
    throw "未找到构建产物: $Arm64Apk"
}

$TargetApk = Join-Path $DistDir "EveryoneNAS-$AppVersion-arm64-v8a-release.apk"
Copy-Item -Path $Arm64Apk -Destination $TargetApk -Force

if ($BuildAab) {
    $AabSource = Join-Path $FlutterDir "build\app\outputs\bundle\release\app-release.aab"
    if (-not (Test-Path $AabSource)) {
        throw "未找到 AAB 构建产物: $AabSource"
    }
    $TargetAab = Join-Path $DistDir "EveryoneNAS-$AppVersion-arm64-v8a-release.aab"
    Copy-Item -Path $AabSource -Destination $TargetAab -Force
}

Write-Host ""
Write-Host "[EveryoneNAS][Android] 发行版构建完成，归档清单:" -ForegroundColor Green
Get-ChildItem -Path $DistDir -File | Format-Table Name, @{Label = "Size(MB)"; Expression = { [math]::Round($_.Length / 1MB, 2) } }, LastWriteTime -AutoSize
Write-Host "调试符号已剥离至: $(Join-Path $FlutterDir 'build\symbols\android')" -ForegroundColor DarkGray
