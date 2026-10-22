//! IBus `org.freedesktop.IBus.Engine`、`Factory` 与 `Service` D-Bus 接口服务实现
//!
//! # 设计原理
//! - **实现初衷**：对接 GNOME (Mutter) 与 `ibus-daemon` 的 D-Bus 通信协议，将系统键盘输入、鼠标选词、
//!   焦点切换与属性栏交互转发至 `IbusSessionCore`，并通过标准信号驱动 GNOME Shell 原生候选框与客户端内联预编辑。
//! - **核心优势**：所有状态变更均在微秒级纯内存同步临界区内完成，信号序列化与异步 I/O 发送全部位于锁作用域之外，
//!   严格杜绝跨 `.await` 持有互斥锁，保证高并发按键流下的极低延迟。
//! - **代价与局限**：依赖 `ibus-daemon` 转发键盘与候选面板事件。

pub mod emitter;
pub mod factory;
pub mod focus;
pub mod navigation;
pub mod properties;
pub mod service;
pub mod snapshot;

pub use factory::{IbusFactoryService, IbusGenericService};
pub use service::IbusEngineService;
pub(crate) use snapshot::UiSnapshot;

/// 微澜 IBus 引擎固定导出的 D-Bus 对象路径
pub const WAYLAN_ENGINE_OBJECT_PATH: &str = "/org/freedesktop/IBus/Engine/1";

/// 微澜 IBus 工厂固定导出的 D-Bus 对象路径
pub const WAYLAN_FACTORY_OBJECT_PATH: &str = "/org/freedesktop/IBus/Factory";
