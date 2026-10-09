# 微澜 (WayLan)

> 微风轻拂起微澜 —— 专为 Wayland 打造的现代轻量纯 Rust 中文输入法。

## 项目架构
- `waylan-core`: 核心基础类型与抽象（按键、候选词、配置规范）
- `waylan-algo`: 现代核心算法包（音节图 DAG、倒排词典、Viterbi 解码、自适应学习、流水线与预处理、词库构建与导入）
- `waylan-ui`: 现代单行胶囊悬浮候选框矢量渲染引擎
- `waylan-wayland`: Wayland 协议客户端（input-method-v1 与 layer-shell）
- `waylan-daemon`: 主程序可执行服务（CLI 入口、后台守护进程、自检与词库管理）

## 测试与构建
```bash
# 全工作区测试
cargo test --workspace

# 格式与代码质量自检
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings

# 启动输入法环境自检
cargo run -p waylan-daemon -- --check
```


