//! 外部词库解析与增量合入领域服务模块
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法提供统一的外部词库（搜狗细胞词库 .scel、Rime YAML、纯文本及辅助词典）
//!   文件解析与领域模型增量合入服务。
//! - **核心优势**：
//!   1. 领域流水线内聚，CLI 与图形设置中心共享同一入口；
//!   2. 统一 64MB 安全限制与格式探测；
//!   3. 零多余数据复制，强类型领域报告。

pub mod parser;
pub mod service;

pub use parser::DictImporter;
pub use service::{AuxImportReport, DictImportReport, DictImportService};
pub use waylan_algo::types::ImportedDictEntry;
