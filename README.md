# 微澜 (WayLan) 拼音输入法

> 微风轻拂起微澜 —— 专为现代 Linux 与 Wayland 环境打造的轻量、高性能纯 Rust 中文拼音输入法。

---

## 项目概述

微澜 (WayLan) 是一款面向现代 Linux 桌面生态（GNOME、Hyprland、Sway、KDE Plasma 等）构建的纯 Rust 现代中文拼音输入法。针对传统 Linux 输入法普遍存在的架构臃肿、GNOME 桌面环境原生数字标签重复与基线对齐跳动、高频击键撕裂与资源开销大等痛点，微澜从算法内核、底层协议栈到矢量渲染层进行了全链路深度重构，旨在提供轻量流畅、响应迅捷且视觉优雅的现代打字体验。

---

## 核心特性亮点

### 1. 完备的双架构协议栈
- **Wayland 原生协议驱动**：
  - 完整实现 `zwp_input_method_v2` 与 `zwp_virtual_keyboard_v1` 原生扩展，支持双缓冲状态挂起与 Done 原子提交；
  - 完整支持 `zwp_input_method_v1` 协议，并通过 `input_panel` 与 `layer_shell` 窗口角色实现无缝向前兼容与自动降级协商。
- **IBus 引擎与 GNOME 原生深度集成**：
  - 基于异步纯 Rust DBus 服务提供标准 IBus 输入法组件后端，原生纳管 GNOME 桌面输入源；
  - 内置专属 GNOME Shell 扩展，彻底消除 GNOME 原生重复数字标签与西文字符纵向对齐悬空落差，实现候选框紧凑微排版与状态无感折叠。
- **共享内存零拷贝复用**：
  - 优先通过 `memfd_create` 建立纯物理内存共享缓冲池（WlShmPool），实现候选框表面双缓冲槽位复用，击键重绘零系统调用与高频零撕裂。
- **动态物理键盘布局适配**：
  - 运行时动态解析宿主机 XKB Keymap 映射，原生适配 Dvorak、Colemak、AZERTY 等各类非 QWERTY 键盘布局。

### 2. 现代工业级拼音算法引擎
- **智能音节分词与切分**：
  - 基于 Prism 动态有向无环图（DAG）分词模型，支持全拼切分、首声母简拼回退与通用声母链跨词短语合成；
  - 完整支持 9 类双向模糊音展开（如 z/zh、c/ch、s/sh、an/ang、en/eng、in/ing 等）。
- **Viterbi 解码与高密度转移矩阵**：
  - 采用基于 Viterbi 动态规划的最优解码管道与动态词频惩罚收敛；
  - 引入基于内存映射（mmap）的 8 位标量量化 CSR（Compressed Sparse Row）高密度 Bigram 二元语言模型，纳秒级局部二分查找，消除稀疏哈希表开销。
- **紧凑型零拷贝词典体系**：
  - 采用双数组 Trie（Double Array Trie, DAT）紧凑数据结构，词库按需分页换入，大幅降低内存驻留。
- **OpenCC 简繁转换**：
  - 纯 Rust 正向最大匹配（FMM）转换引擎，搭载两级词表，支持零堆分配的简繁体中文平滑转换。
- **用户自适应学习与 WAL 持久化**：
  - 内置写前日志（Write-Ahead Log, WAL）与双阶段自动压缩去重机制，选词记忆与用户自造词实时安全刷盘，系统异常退出数据零丢失。

### 3. 轻量离屏矢量渲染与多形态候选框
- **纯 CPU 离屏矢量渲染**：
  - 基于 `tiny-skia` 离屏软光栅化与 `cosmic-text` 多语言排版引擎，无重型图形框架依赖，跨驱动与无 GPU 环境稳定呈现；
  - 内置字形位图 LRU 高速缓存与 HiDPI 物理像素无损缩放适配，字体坐底严格平齐。
- **多形态交互候选视图**：
  - **单行胶囊候选栏**：清爽利落的主输入视图，支持四分之一窄空格微排版与选词高亮胶囊；
  - **二级展开浮层卡片**：针对扩展候选项（如时间日期多格式、大写金额多类型）提供精准二级浮层卡片；
  - **五行网格大候选面板**：支持倒三角点击或快捷键展开大网格多候选词快速检索。
- **多样化序号排版风格**：
  - 支持现代空白风（默认纯数字加半角空格）、经典点号风（1. ~ 9.）、圆圈序号风（① ~ ⑩）、括号风（[1] / (1)）及极简无序号模式；
  - 序号与文本在排版层一体化合成，彻底根除基线垂直错位。
- **优雅主题定制**：
  - 内置浅色白底翡翠绿主题与深色夜间模式，支持边框圆角、内边距与字体样式热重载。

### 4. 智能交互与扩展模式
- **智能舒适标点细节**：
  - 支持斜杠自动转中文顿号（/ 自动转 、）；
  - 成对括号与引号输入时自动闭合，光标自适应左移回退；
  - 数字输入保护：打数字（如 12:30 或 3.14）时冒号、小数点与斜杠自动保持英文半角，无需手动切换模式。
- **动态快捷扩展模式**：
  - **V 模式**：输入 `v` 引导，支持实时四则运算表达式求值（支持括号与优先级），以及人民币财务大写金额与规范大写数字一键生成；
  - **U 模式**：支持拆字拼字输入（如输入 `u/huo/huo/huo` 联想“焱”）；
  - **I 模式**：常用特殊符号、罗马数字与数学单位快捷输入；
  - **动态时间日期**：输入 `riqi`、`shijian`、`rq` 动态展开全量中文日期、时间戳与农历干支纪年格式。

### 5. 图形化偏好设置中心与桌面托盘
- **Slint 现代化设置中心**：
  - 提供分栏式图形配置界面（`waylan-settings`），支持快捷键绑定、界面主题、候选词排版风格与外部词库导入；
  - 外部词库解析与合并迁移至后台异步线程执行，界面操作完全无阻塞。
- **跨桌面系统托盘集成**：
  - 基于 DBusMenu / StatusNotifierItem (SNI) 规范常驻系统顶栏/托盘，实时双向同步中英文与大写锁定状态；
  - 配置文件变更（inotify）与词库更新无感热感知重载。

---

## 模块架构

工作区采用清晰的分层解耦多包架构：

| 包名 | 职责说明 |
| :--- | :--- |
| `crates/waylan-core` | 核心数据模型、配置定义、按键事件抽象与候选词一体化排版模块 |
| `crates/waylan-algo` | 拼音分词切分、Viterbi解码、CSR Bigram转移矩阵、DAT词典与WAL自适应模型 |
| `crates/waylan-ui` | tiny-skia 离屏矢量渲染器、cosmic-text 排版、HiDPI缩放与多形态候选框面板 |
| `crates/waylan-wayland` | Wayland 双协议实现（input-method v1/v2、virtual-keyboard 与 shm 共享内存） |
| `crates/waylan-daemon` | 守护进程服务、IBus 引擎集成、DBusMenu/SNI 系统托盘与热重载调度 |
| `crates/waylan-settings` | 基于 Slint 框架构建的现代化跨桌面图形偏好设置中心 |

---

## 编译与环境依赖

### 1. 系统开发库依赖
在 Debian / Ubuntu 及其衍生发行版上，编译前需安装以下基础构建库：

```bash
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libwayland-dev \
    libxkbcommon-dev \
    libfontconfig1-dev \
    libfreetype6-dev \
    libdbus-1-dev
```

### 2. 源码编译与本地运行
```bash
# 克隆仓库
git clone https://github.com/mangerle/waylan.git
cd waylan

# 运行全工作区单元测试与集成测试
cargo test --workspace

# 编译所有 Release 二进制产物
cargo build --release

# 运行主守护进程
cargo run --release -p waylan-daemon

# 运行图形设置中心
cargo run --release -p waylan-settings
```

---

## Debian 打包与系统服务

本项目已完整集成 `cargo-deb` 打包流水线与 systemd 用户服务单元：

### 1. 构建 Debian 安装包
```bash
# 调用内置打包脚本一键编译并生成 .deb 软件包
./scripts/build-deb.sh

# 或打包完成后直接自动安装
./scripts/build-deb.sh --install
```

打包完成后，生成的软件包位于 `target/debian/waylan_<版本>_<架构>.deb`。

### 2. systemd 用户级服务管理
安装后，微澜作为 systemd 用户级服务运行，可在开机进入桌面会话时自动拉起：

```bash
# 启用开机自启
systemctl --user enable waylan.service

# 启动服务
systemctl --user start waylan.service

# 查看运行状态与实时日志
systemctl --user status waylan.service
journalctl --user -u waylan.service -f
```

---

## 常用操作与使用技巧

- **中英文切换**：单击 `Shift` 键（或通过设置中心绑定为 `Caps Lock`）；
- **候选词选择**：数字键 `1` ~ `9`（第 10 项为 `0`），或使用空格键确认当前高亮项；
- **候选词翻页**：使用 `-` / `=`、`[` / `]`、`PageUp` / `PageDown` 或方向键 `上` / `下`；
- **展开大网格面板**：候选状态下按方向键 `下`（或鼠标点击候选框右侧倒三角展开按钮）；
- **动态日期时间**：拼音输入 `riqi`、`shijian` 或 `rq`，按 `Tab` 键即可在二级展开卡片中挑选指定格式；
- **算术计算器**：输入 `v1+2*3`，候选第一项即显示运算结果 `7`；
- **财务金额大写**：输入 `v1234.56`，直接上屏 `壹仟贰佰叁拾肆元伍角陆分`。

---

## 代码规范与自检门禁

本项目遵循严苛的代码质量与工程准则（详见 [AGENTS.md](AGENTS.md)）：

```bash
# 代码风格格式化检查
cargo fmt --all -- --check

# 静态代码分析门禁（零告警策略）
cargo clippy --all-targets -- -D warnings

# 全工作区测试
cargo test --workspace
```

---

## 开源许可

本项目遵循 [Apache-2.0 许可证](LICENSE-APACHE) 双重开源许可。
