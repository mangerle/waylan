# 微澜 (WayLan)

> 微风轻拂起微澜 —— 专为 Wayland 打造的现代轻量纯 Rust 中文输入法。

## 特性亮点
- **协议完备**：同时支持 `zwp_input_method_v2`（推荐，现代平滑输入体验）与 `zwp_input_method_v1`（向前兼容），具备智能自动降级机制。
- **动态布局适配**：从 XKB Keymap 运行时动态解析物理按键符号映射，完美适配 Dvorak、Colemak、AZERTY 等非 QWERTY 键盘布局。
- **现代拼音算法**：基于音节 DAG 有向无环图、Double Array Trie（DAT）、Viterbi 解码与二元语言模型（Bigram CSR 标量量化）。
- **用户自适应与 WAL**：集成写前日志（Write-Ahead Log）与自动平滑合并压缩机制，保障用户个性化词频实时落盘与持久化。
- **现代化矢量渲染**：基于 `fontdue` 软光栅化与 Wayland SHM 共享内存双缓冲，呈现优雅的胶囊拟物悬浮候选窗口。
- **多功能扩展模式**：原生内置 V 模式（实时四则运算与大写金额转换）、U 模式（拆字组词输入）与 I 模式（常用特殊符号快捷输入）。

## 项目架构
- `waylan-core`: 核心基础类型与抽象（按键、候选词、状态机与输入法特型契约）。
- `waylan-algo`: 现代核心拼音算法库（音节切分、紧凑词库、Viterbi 解码、纠错与自适应学习）。
- `waylan-ui`: 现代单行胶囊悬浮候选框矢量渲染引擎，内置字体加载与双缓冲。
- `waylan-wayland`: Wayland 协议交互层（支持 V1/V2 input-method、virtual-keyboard 与 layer-shell）。
- `waylan-daemon`: 主程序可执行服务（CLI 入口、后台常驻守护、DBus 系统托盘与设置界面）。

## 测试与构建
```bash
# 全工作区测试
cargo test --workspace

# 格式与代码质量自检
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings

# 启动输入法环境自检
cargo run -p waylan-daemon -- --check

# 启动主输入法守护进程
cargo run -p waylan-daemon
```



