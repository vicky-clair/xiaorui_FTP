<#
.SYNOPSIS
    xiaorui_FTP Windows x64 一键测试与 Release 正式安装包构建脚本
.DESCRIPTION
    依次执行：
    1. Rust 静态检查（Clippy）与单元测试
    2. TypeScript 类型检查与 Vite 前端生产构建
    3. Tauri v2 Windows x64 Release 打包（生成 NSIS 与 MSI 安装包）
    4. 输出全部安装包路径、体积与 SHA-256 校验值
#>

param(
    [ValidateSet("x86_64-pc-windows-msvc")]
    [string]$Target = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"

Write-Host "==> [1/4] 运行 Rust Clippy 与单元测试 ($Target)..." -ForegroundColor Cyan
Push-Location "$PSScriptRoot\..\src-tauri"
try {
    cargo clippy --target $Target --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    cargo test --target $Target
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}

Write-Host "==> [2/4] 构建 TypeScript + Vite 生产前端..." -ForegroundColor Cyan
Push-Location "$PSScriptRoot\.."
try {
    npm run build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "==> [3/4] 打包 Tauri v2 Windows x64 Release 安装包 (NSIS / MSI)..." -ForegroundColor Cyan
    cargo tauri build --target $Target
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "==> [4/4] Release 安装包构建完成：" -ForegroundColor Green
    $bundleDir = Join-Path $PSScriptRoot "..\src-tauri\target\$Target\release\bundle"
    if (Test-Path $bundleDir) {
        Get-ChildItem -Recurse -File $bundleDir -Include *.exe,*.msi | ForEach-Object {
            $hash = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash
            Write-Host ("  - {0} ({1:N2} MB)" -f $_.FullName, ($_.Length / 1MB))
            Write-Host ("    SHA-256: {0}" -f $hash)
        }
    }
} finally {
    Pop-Location
}
