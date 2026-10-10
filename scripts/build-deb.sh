#!/usr/bin/env bash
# 微澜 (WayLan) 原生 Debian (.deb) 软件包构建脚本
# 本脚本遵循 Debian 打包标准规范，输出开箱即用的 waylan_0.1.0_amd64.deb 包。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PKG_NAME="waylan"
PKG_VERSION="$(grep -m1 '^version' "${PROJECT_ROOT}/Cargo.toml" | tr -d ' "' | cut -d'=' -f2)"
PKG_ARCH="amd64"
DEB_DIR_NAME="${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}"
BUILD_ROOT="${PROJECT_ROOT}/target/debian/${DEB_DIR_NAME}"
OUTPUT_DEB="${PROJECT_ROOT}/target/debian/${DEB_DIR_NAME}.deb"

echo "[微澜打包] 步骤 1/4: 编译 Release 优化版本二进制程序..."
cd "${PROJECT_ROOT}"
cargo build --release --bin waylan --bin waylan-settings

echo "[微澜打包] 步骤 2/4: 构建 Debian 文件系统标准目录树..."
rm -rf "${PROJECT_ROOT}/target/debian"
mkdir -p "${PROJECT_ROOT}/target/debian"

# 创建目录结构
mkdir -p "${BUILD_ROOT}/DEBIAN"
mkdir -p "${BUILD_ROOT}/usr/bin"
mkdir -p "${BUILD_ROOT}/usr/share/waylan/data"
mkdir -p "${BUILD_ROOT}/usr/share/waylan/assets"
mkdir -p "${BUILD_ROOT}/usr/share/icons/hicolor/scalable/apps"
mkdir -p "${BUILD_ROOT}/usr/share/applications"
mkdir -p "${BUILD_ROOT}/usr/lib/systemd/user"

# 1. 安装二进制可执行文件
cp "${PROJECT_ROOT}/target/release/waylan" "${BUILD_ROOT}/usr/bin/waylan"
cp "${PROJECT_ROOT}/target/release/waylan-settings" "${BUILD_ROOT}/usr/bin/waylan-settings"
chmod 755 "${BUILD_ROOT}/usr/bin/waylan"
chmod 755 "${BUILD_ROOT}/usr/bin/waylan-settings"

# 2. 安装全量字词库、辅助混输词库与简繁转换词库 (到 /usr/share/waylan/data/)
cp "${PROJECT_ROOT}/data/base.dict.bin" "${BUILD_ROOT}/usr/share/waylan/data/base.dict.bin"
chmod 644 "${BUILD_ROOT}/usr/share/waylan/data/base.dict.bin"
if [ -f "${PROJECT_ROOT}/data/aux.dict.bin" ]; then
    cp "${PROJECT_ROOT}/data/aux.dict.bin" "${BUILD_ROOT}/usr/share/waylan/data/aux.dict.bin"
    chmod 644 "${BUILD_ROOT}/usr/share/waylan/data/aux.dict.bin"
fi
if [ -f "${PROJECT_ROOT}/data/opencc.dict.bin" ]; then
    cp "${PROJECT_ROOT}/data/opencc.dict.bin" "${BUILD_ROOT}/usr/share/waylan/data/opencc.dict.bin"
    chmod 644 "${BUILD_ROOT}/usr/share/waylan/data/opencc.dict.bin"
fi
# 同时安装 8 大类原始分类词库模板以备设置中心与用户扩展使用
if [ -d "${PROJECT_ROOT}/data/dicts" ]; then
    mkdir -p "${BUILD_ROOT}/usr/share/waylan/dicts"
    cp -r "${PROJECT_ROOT}/data/dicts/"* "${BUILD_ROOT}/usr/share/waylan/dicts/"
fi

# 3. 安装矢量资源与系统图标
cp "${PROJECT_ROOT}/assets/"*.svg "${BUILD_ROOT}/usr/share/waylan/assets/"
chmod 644 "${BUILD_ROOT}/usr/share/waylan/assets/"*.svg
cp "${PROJECT_ROOT}/assets/waylan.svg" "${BUILD_ROOT}/usr/share/icons/hicolor/scalable/apps/waylan.svg"
chmod 644 "${BUILD_ROOT}/usr/share/icons/hicolor/scalable/apps/waylan.svg"

# 4. 安装桌面入口规范文件 (.desktop)
cp "${PROJECT_ROOT}/assets/waylan.desktop" "${BUILD_ROOT}/usr/share/applications/waylan.desktop"
cp "${PROJECT_ROOT}/assets/waylan-settings.desktop" "${BUILD_ROOT}/usr/share/applications/waylan-settings.desktop"
chmod 644 "${BUILD_ROOT}/usr/share/applications/"*.desktop

# 5. 安装 systemd 用户服务单元
cp "${PROJECT_ROOT}/systemd/user/waylan.service" "${BUILD_ROOT}/usr/lib/systemd/user/waylan.service"
chmod 644 "${BUILD_ROOT}/usr/lib/systemd/user/waylan.service"

echo "[微澜打包] 步骤 3/4: 生成 DEBIAN 包控制元数据与安装触发钩子..."

# 写入 DEBIAN/control
cat << EOF > "${BUILD_ROOT}/DEBIAN/control"
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Section: utils
Priority: optional
Architecture: ${PKG_ARCH}
Maintainer: WayLan Contributors <waylan@localhost>
Depends: libc6 (>= 2.34)
Description: 专为 Wayland 打造的现代轻量纯 Rust 中文输入法
 微澜 (WayLan) 是一款面向现代 Linux Wayland 合成器（Hyprland、Sway、KDE、GNOME）
 设计的轻量级中文拼音输入法守护进程。具备智能算法、首字母简拼、剪贴板联动、
 日期时间二级展开和图形化偏好设置中心。
EOF

# 写入 DEBIAN/postinst (安装后钩子)
cat << 'EOF' > "${BUILD_ROOT}/DEBIAN/postinst"
#!/bin/sh
set -e

if [ "$1" = "configure" ]; then
    # 刷新图标缓存
    if which gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t /usr/share/icons/hicolor >/dev/null 2>&1 || true
    fi

    # 刷新桌面数据库
    if which update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q /usr/share/applications >/dev/null 2>&1 || true
    fi

    echo "============================================================"
    echo "  微澜 (WayLan) 中文输入法已成功安装！"
    echo "  - 启动输入法守护进程: systemctl --user enable --now waylan.service"
    echo "  - 打开偏好设置窗口:   waylan-settings"
    echo "============================================================"
fi
exit 0
EOF
chmod 755 "${BUILD_ROOT}/DEBIAN/postinst"

# 写入 DEBIAN/prerm (卸载前钩子)
cat << 'EOF' > "${BUILD_ROOT}/DEBIAN/prerm"
#!/bin/sh
set -e

if [ "$1" = "remove" ]; then
    # 尝试通知当前图形会话停止服务
    systemctl --user stop waylan.service >/dev/null 2>&1 || true
fi
exit 0
EOF
chmod 755 "${BUILD_ROOT}/DEBIAN/prerm"

echo "[微澜打包] 步骤 4/4: 调用 dpkg-deb 打包生成 .deb 安装文件..."
dpkg-deb --build --root-owner-group "${BUILD_ROOT}" "${OUTPUT_DEB}"

echo "============================================================"
echo "[打包完成] 成功构建 Debian 软件包:"
echo "  文件路径: ${OUTPUT_DEB}"
echo "  安装命令: sudo dpkg -i ${OUTPUT_DEB}"
echo "============================================================"
