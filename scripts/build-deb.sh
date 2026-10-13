#!/usr/bin/env bash
# 微澜 (WayLan) 原生 Debian (.deb) 软件包构建脚本
# 本脚本遵循 Debian 打包标准规范，输出开箱即用的 waylan_0.1.0_amd64.deb 包。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PKG_NAME="waylan"
PKG_VERSION="$(grep -m1 '^version' "${PROJECT_ROOT}/Cargo.toml" | tr -d ' "' | cut -d'=' -f2)"
PKG_ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
DEB_DIR_NAME="${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}"
BUILD_ROOT="${PROJECT_ROOT}/target/debian/${DEB_DIR_NAME}"
OUTPUT_DEB="${PROJECT_ROOT}/target/debian/${DEB_DIR_NAME}.deb"

echo "[微澜打包] 步骤 1/3: 编译 Release 优化版本二进制程序 (waylan & waylan-settings)..."
cd "${PROJECT_ROOT}"
cargo build --release --bin waylan --bin waylan-settings

echo "[微澜打包] 步骤 2/3: 检查 cargo-deb 打包工具..."
if ! command -v cargo-deb >/dev/null 2>&1; then
    echo "未检测到 cargo-deb，正在通过 cargo 自动安装..."
    cargo install cargo-deb
fi

echo "[微澜打包] 步骤 3/3: 调用 cargo-deb 构建原生 Debian 软件包..."
cargo deb -p waylan-daemon --no-build

echo "============================================================"
echo "[打包完成] 成功构建 Debian 软件包:"
echo "  生成目录: ${PROJECT_ROOT}/target/debian/"
echo "  安装命令: sudo dpkg -i ${PROJECT_ROOT}/target/debian/waylan_*.deb"
echo "============================================================"
