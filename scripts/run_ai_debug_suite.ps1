#Requires -Version 5.1
<#
.SYNOPSIS
  一键编译 lanvault-ffi 并运行 AI 全功能自动化调试套件，自动产出 JSON 诊断报告与探针日志。
#>
param(
    [switch]$SkipBuild,
    [string]$ReportDir = "debug_reports"
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $RepoRoot

try {
    if (-not $SkipBuild) {
        Write-Host "[1/2] 正在编译 lanvault-ffi 与 lanvault-debug-cli..." -ForegroundColor Cyan
        cargo build --release -p lanvault-ffi --bin lanvault_debug_cli --lib
    }

    New-Item -ItemType Directory -Force -Path $ReportDir | Out-Null
    $stamp = Get-Date -Format "yyyyMMdd_HHmmss"
    $reportFile = Join-Path $ReportDir "ai_debug_report_$stamp.json"

    Write-Host "[2/2] 正在运行全功能 AI 自动化调试套件..." -ForegroundColor Cyan
    python scripts/ai_debug_runner.py suite --report $reportFile
    $code = $LASTEXITCODE

    Write-Host "诊断报告已保存至: $reportFile" -ForegroundColor Green
    exit $code
}
finally {
    Pop-Location
}
