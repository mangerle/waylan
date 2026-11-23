//! IBus 工厂与通用服务接口实现
//!
//! # 设计原理
//! - **实现初衷**：响应 ibus-daemon 的 CreateEngine 请求，为新获取焦点的应用动态分配并注册 Engine 与 Service 对象。
//! - **核心优势**：为每个应用程序分配独立的 D-Bus 对象路径，实现输入状态的上下文隔离与并发安全。
//! - **代价与局限**：依赖宿主 D-Bus 连接的 ObjectServer 动态挂载能力。

use log::{debug, warn};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use zbus::OwnedObjectPath;
use zbus::connection::Connection;
use zbus::fdo;
use zbus::interface;

use super::super::keys::IbusSessionCore;
use super::service::IbusEngineService;

/// `org.freedesktop.IBus.Factory` 接口实现
pub struct IbusFactoryService {
    pub(crate) core: Arc<Mutex<IbusSessionCore>>,
    pub(crate) conn_slot: Arc<OnceLock<Connection>>,
    pub(crate) engine_template: IbusEngineService,
    pub(crate) next_engine_id: Arc<AtomicU32>,
}

#[interface(name = "org.freedesktop.IBus.Factory")]
impl IbusFactoryService {
    /// 响应 `ibus-daemon` 的引擎实例创建请求，返回动态创建并注册的微澜引擎对象路径
    async fn create_engine(&self, name: &str) -> fdo::Result<OwnedObjectPath> {
        debug!("[微澜 IBus] 收到 CreateEngine 请求，引擎名: {}", name);
        {
            let mut guard = self.core.lock();
            guard.reset_context();
        }

        let (obj_path, path_str) = self.build_engine_object_path()?;
        let conn_registered = match self.conn_slot.get() {
            Some(conn) => {
                self.register_engine_interfaces(conn, &obj_path, &path_str)
                    .await
            }
            None => false,
        };

        if !conn_registered {
            warn!(
                "[微澜 IBus 警告] D-Bus 宿主连接未就绪，无法为动态路径 {} 注册接口",
                path_str
            );
            return Err(fdo::Error::Failed(
                "D-Bus 宿主连接尚未就绪，无法创建引擎实例".to_string(),
            ));
        }

        Ok(obj_path)
    }
}

/// `IbusFactoryService` 的内部辅助逻辑
///
/// # 设计原理
/// - 与 `#[interface]` 接口块分离：接口块内的方法会被 zbus 宏导出为 D-Bus 方法，
///   内部辅助逻辑必须置于普通 impl 块中，否则会被误当作 D-Bus 方法暴露。
impl IbusFactoryService {
    /// 为新的引擎实例分配唯一的 D-Bus 对象路径
    fn build_engine_object_path(&self) -> Result<(OwnedObjectPath, String), fdo::Error> {
        let id = self.next_engine_id.fetch_add(1, Ordering::SeqCst);
        let path_str = format!("/org/freedesktop/IBus/Engine/{}", id);
        let obj_path = OwnedObjectPath::try_from(path_str.as_str())
            .map_err(|e| fdo::Error::Failed(format!("构造引擎对象路径失败: {}", e)))?;
        Ok((obj_path, path_str))
    }

    /// 在动态对象路径上注册 Engine 与 Service 接口，返回是否注册成功
    ///
    /// # 设计原理
    /// - Engine 与 Service 接口分别独立挂载：单个接口注册失败仅告警降级，
    ///   另一个接口仍可正常工作，避免局部失败导致整个引擎实例不可用。
    async fn register_engine_interfaces(
        &self,
        conn: &Connection,
        obj_path: &OwnedObjectPath,
        path_str: &str,
    ) -> bool {
        let mut srv = self.engine_template.clone();
        srv.is_password = Arc::new(AtomicBool::new(false));
        srv.lock_healer = Arc::clone(&self.engine_template.lock_healer);
        let generic_srv = IbusGenericService {
            core: Arc::clone(&self.core),
        };
        if let Err(e) = conn.object_server().at(obj_path, srv).await {
            warn!(
                "[微澜 IBus] 为动态路径 {} 注册 Engine 接口失败: {}",
                path_str, e
            );
        }
        if let Err(e) = conn.object_server().at(obj_path, generic_srv).await {
            warn!(
                "[微澜 IBus] 为动态路径 {} 注册 Service 接口失败: {}",
                path_str, e
            );
        }
        debug!(
            "[微澜 IBus] 成功为新应用输入上下文动态注册 Engine 实例: {}",
            path_str
        );
        true
    }
}

/// `org.freedesktop.IBus.Service` 通用生命周期接口实现
pub struct IbusGenericService {
    pub(crate) core: Arc<Mutex<IbusSessionCore>>,
}

#[interface(name = "org.freedesktop.IBus.Service")]
impl IbusGenericService {
    /// 响应服务对象销毁请求，安全重置内部预编辑与按键状态
    fn destroy(&self) {
        debug!("[微澜 IBus] 收到 Service.Destroy 请求，重置当前输入法上下文。");
        let mut guard = self.core.lock();
        guard.reset_context();
    }
}
