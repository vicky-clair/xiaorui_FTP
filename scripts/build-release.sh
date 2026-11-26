#!/usr/bin/env bash
# xiaorui_FTP macOS / Linux x64 一键测试与 Release 正式安装包构建脚本
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OS="$(uname -s)"

if [[ "$OS" == "Darwin" ]]; then
  TARGET="x86_64-apple-darwin"
elif [[ "$OS" == "Linux" ]]; then
  TARGET="x86_64-unknown-linux-gnu"
else
  echo "不支持的平台: $OS（Windows 请使用 scripts/build-release.ps1）" >&2
  exit 1
fi

echo "==> [1/4] 运行 Rust 格式、Clippy 与单元测试 ($TARGET)..."
(
  cd "$ROOT_DIR/src-tauri"
  cargo fmt -- --check
  cargo clippy --target "$TARGET" --all-targets -- -D warnings
  cargo test --target "$TARGET"
)

echo "==> [2/4] 构建 TypeScript + Vite 生产前端..."
(
  cd "$ROOT_DIR"
  npm run build
  echo "==> [3/4] 打包 Tauri v2 Release x64 安装包..."
  npx tauri build --target "$TARGET"
)

echo "==> [4/4] 构建完成，产物位于 src-tauri/target/$TARGET/release/bundle/"
