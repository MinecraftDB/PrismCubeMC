//! PrismCubeMC 核心模块
//! 
//! 包含服务器的基础设施：
//! - config: 配置管理（兼容原版 server.properties）
//! - launch: 启动管理器（进程生命周期主控）

pub mod config;
pub mod launch;
