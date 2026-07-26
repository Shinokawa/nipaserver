//! nipa-core：领域模型、服务器配置、事件类型（EventMsg）。
//!
//! 回流约束（开发文档 §3.1）：本 crate 经 flutter_rust_bridge 回流 NipaPlay 客户端，
//! **不得依赖 axum/tower**，保持纯逻辑。

pub mod config;
pub mod event;
pub mod model;

pub use config::{ModelSection, ServerConfig};
pub use event::EventMsg;
