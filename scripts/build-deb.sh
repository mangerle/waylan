#!/usr/bin/env bash
# 微澜 (WayLan) 原生 Debian (.deb) 软件包构建脚本
# 本脚本遵循 Debian 打包标准规范，输出开箱即用的 waylan_<版本>_<架构>.deb 原生软件包。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PKG_NAME="waylan"
PKG_VERSION="$(grep -m1 '^version' "${PROJECT_ROOT}/Cargo.toml" | tr -d ' "' | cut -d'=' -f2)"
PKG_ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
DEB_DIR_NAME="${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}"
BUILD_ROOT="${PROJECT_ROOT}/target/debian/${DEB_DIR_NAME}"
OUTPUT_DEB="${PROJECT_ROOT}/target/debian/${DEB_DIR_NAME}.deb"

DO_INSTALL=false
for arg in "$@"; do
    case "$arg" in
        -i|--install)
            DO_INSTALL=true
            ;;
        -h|--help)
            echo "微澜 (WayLan) Debian 软件包构建工具"
            echo "用法: $0 [选项]"
            echo ""
            echo "选项:"
            echo "  -i, --install   打包完成后自动调用 sudo dpkg -i 安装"
            echo "  -h, --help      显示帮助信息"
            exit 0
            ;;
    esac
done

echo "[微澜打包] 步骤 1/4: 检查维护者安装脚本权限规范..."
chmod +x "${PROJECT_ROOT}/debian/postinst" "${PROJECT_ROOT}/debian/prerm"

echo "[微澜打包] 步骤 2/4: 编译 Release 优化版本二进制程序 (waylan & waylan-settings)..."
cd "${PROJECT_ROOT}"
cargo build --release --bin waylan --bin waylan-settings

echo "[微澜打包] 步骤 3/4: 检查 cargo-deb 打包工具与动态生成组件元数据..."
if ! command -v cargo-deb >/dev/null 2>&1; then
    echo "未检测到 cargo-deb，正在通过 cargo 自动安装..."
    cargo install cargo-deb
fi

# 动态生成 IBus 组件描述元数据至构建输出目录 (target/generated/ibus/waylan.xml)，彻底杜绝污染源码仓库
mkdir -p "${PROJECT_ROOT}/target/generated/ibus"
"${PROJECT_ROOT}/target/release/waylan" --ibus-component-xml > "${PROJECT_ROOT}/target/generated/ibus/waylan.xml"

echo "[微澜打包] 步骤 4/4: 调用 cargo-deb 构建原生 Debian 软件包..."
cargo deb -p waylan-daemon --no-build --deb-revision ""

LATEST_DEB="$(ls -t "${PROJECT_ROOT}/target/debian/"*.deb 2>/dev/null | head -n1 || true)"
DEB_SIZE="$(du -h "${LATEST_DEB}" 2>/dev/null | cut -f1 || echo "未知")"

echo "============================================================"
echo "[打包完成] 成功构建 Debian 软件包:"
echo "  软件包路径: ${LATEST_DEB} (体积: ${DEB_SIZE})"
echo "  手动安装:   sudo dpkg -i ${LATEST_DEB}"
echo "============================================================"

if [ "${DO_INSTALL}" = true ]; then
    echo "[自动安装] 正在执行 sudo dpkg -i ${LATEST_DEB}..."
    sudo dpkg -i "${LATEST_DEB}"
fi
