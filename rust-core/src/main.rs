//! PrismCubeMC 服务器入口
//! 
//! 【极简启动】
//! 1. 创建默认配置（或从配置文件加载）
//! 2. 创建 LaunchManager
//! 3. 调用 start()，阻塞直到服务器关闭

use prismcubemc::core::config::ServerConfig;
use prismcubemc::core::launch::LaunchManager;

#[tokio::main]
async fn main() {
    // 创建默认配置
    // TODO: 后续支持从命令行参数指定配置文件路径
    // TODO: 支持 --generate-config 生成默认配置模板
    let config = ServerConfig::default();
    
    // 创建启动管理器
    // 此时生成 session_token，作为本次运行的唯一签证
    let mut launcher = LaunchManager::new(config);
    
    // 启动服务器（阻塞直到收到 Ctrl+C 或致命错误）
    // 内部按顺序初始化：日志 → 横幅 → 进程管理器 → 网络分发器 → 信号监听 → 主循环
    launcher.start().await;
}
