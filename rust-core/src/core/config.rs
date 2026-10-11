//! 服务器配置：PrismCubeMC 的核心运行参数
//! 
//! 【设计说明】
//! ServerConfig 的字段命名严格兼容原版 Minecraft server.properties，
//! 将原版的驼峰/小写命名转换为 Rust 的下划线命名风格。
//! 
//! 例如原版：
//!   server-port=19132
//!   online-mode=true
//!   level-name=world
//! 
//! 对应 Rust：
//!   server_port: 19132
//!   online_mode: true
//!   level_name: "world".to_string()
//! 
//! 这样运维人员可以直接沿用原版配置习惯，降低迁移成本。

use std::path::Path;

/// 服务器配置结构体
/// 
//! 【原版兼容字段】
//! 这些字段与 Minecraft server.properties 一一对应，
//! 确保 PrismCubeMC 对原版运维人员零学习成本。
/// 
/// 【专属扩展字段】
/// 以 prism_ 或特定功能前缀命名，避免与原版字段冲突。
#[derive(Debug, Clone)]
pub struct ServerConfig {
    // ═══════════════════════════════════════════════════════════
    // 原版 server.properties 兼容字段
    // ═══════════════════════════════════════════════════════════
    
    /// server-port：服务器监听端口
    /// 
    /// 原版默认值 25565（Java），但 PrismCubeMC 使用 19132（基岩版默认）
    /// 作为 Prism 单端口汇聚点，同时接收 TCP（Java）和 UDP（基岩版）
    pub server_port: u16,
    
    /// motd：服务器列表中显示的消息
    /// 
    /// 原版支持颜色代码 §，PrismCubeMC 额外支持根据客户端版本
    /// 自动选择协议格式发送（旧版纯文本 / 新版 JSON）
    pub motd: String,
    
    /// max-players：最大同时在线玩家数
    /// 
    /// 项目目标日活 50 人，此值建议设为 20-100 范围
    pub max_players: u32,
    
    /// gamemode：默认游戏模式
    /// 
    /// 原版值：survival, creative, adventure, spectator
    /// 后续需要映射为内部枚举
    pub gamemode: String,
    
    /// difficulty：游戏难度
    /// 
    /// 原版值：peaceful, easy, normal, hard
    /// 影响怪物生成、伤害计算等
    pub difficulty: String,
    
    /// online-mode：是否启用正版验证
    /// 
    /// true：连接 Mojang 验证服务器，检查玩家 UUID
    /// false：离线模式，允许盗版连接（开发测试用）
    /// 
    /// 【红线】即使为 false，内部仍生成有效 UUID，绝不内存伪造
    pub online_mode: bool,
    
    /// level-name：主世界存档目录名
    /// 
    /// 原版默认 "world"，PrismCubeMC 沿用
    /// 存档格式后续需要兼容 Anvil（Java）和 LevelDB（基岩版）
    pub level_name: String,
    
    // ═══════════════════════════════════════════════════════════
    // PrismCubeMC 专属字段
    // ═══════════════════════════════════════════════════════════
    
    /// java_proxy_path：Java 代理进程的可执行文件路径
    /// 
    /// 用于启动 Java IPC 桥接进程，运行 ViaVersion / ProtocolSupport
    /// 例如："engines/prism-java-proxy.jar"
    pub java_proxy_path: String,
    
    /// data_dict_path：数据字典目录路径
    /// 
    /// 包含各版本协议定义、方块映射表、物品 ID 对照等
    /// 由 Python 离线工具从 minecraft-data 提取生成
    /// 例如："data/dict"
    pub data_dict_path: String,
}

/// 提供合理的默认值，与原版 Minecraft 服务器保持一致性
/// 
/// 【注意】生产环境应从 server.properties 或 prism.toml 加载，
/// 而非依赖默认值。默认值仅用于首次启动生成配置模板。
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            // Prism 单端口汇聚：19132（基岩版默认端口）
            // 同时接收 Java（TCP）和基岩版（UDP）连接
            server_port: 19132,
            
            // 默认 MOTD，显示服务器名称和版本提示
            // §a 为绿色，§r 为重置，后续根据客户端版本翻译格式
            motd: "§aPrismCubeMC§r — 单端口 · 多语言 · 全版本".to_string(),
            
            // 项目目标日活 50 人，最大并发设为 100 留有余量
            max_players: 100,
            
            // 默认生存模式，最通用的游戏方式
            gamemode: "survival".to_string(),
            
            // 普通难度，平衡挑战与可玩性
            difficulty: "normal".to_string(),
            
            // 默认启用正版验证，生产环境推荐
            // 开发测试时可设为 false，但绝不跳过 UUID 生成
            online_mode: true,
            
            // 沿用原版默认存档名
            level_name: "world".to_string(),
            
            // Java IPC 桥接进程路径，相对项目根目录
            java_proxy_path: "engines/prism-java-proxy.jar".to_string(),
            
            // 数据字典根目录，由 Python 构建工具生成
            data_dict_path: "data/dict".to_string(),
        }
    }
}

impl ServerConfig {
    /// 从配置文件加载（骨架实现）
    /// 
    /// 【设计意图】
    /// 后续支持两种配置来源：
    /// 1. server.properties：原版兼容格式，key=value 每行一个
    /// 2. prism.toml：PrismCubeMC 扩展配置，支持嵌套结构
    /// 
    /// 【加载优先级】
    /// 命令行参数 > 环境变量 > prism.toml > server.properties > Default
    /// 
    /// 【参数】
    /// - path: 配置文件路径，通常为 "server.properties" 或 "prism.toml"
    /// 
    /// 【返回值】
    /// - Ok(ServerConfig)：成功解析
    /// - Err：文件不存在或格式错误
    pub fn load_from_file<P: AsRef<Path>>(_path: P) -> Result<Self, std::io::Error> {
        // TODO: 实现配置文件解析
        // 
        // 步骤 1：检测文件扩展名，选择解析器
        //   - .properties → 原版 key=value 格式
        //   - .toml → TOML 格式（需要 toml crate）
        //   - .json → JSON 格式
        // 
        // 步骤 2：逐行/逐键解析，未知字段记录警告但不报错
        // 
        // 步骤 3：验证字段合法性
        //   - server_port: 1-65535
        //   - max_players: > 0
        //   - gamemode: 必须是已知模式之一
        //   - java_proxy_path: 文件必须存在
        //   - data_dict_path: 目录必须存在
        
        // 骨架阶段：返回默认配置，后续实现真实加载
        Ok(Self::default())
    }
    
    /// 保存配置到文件（骨架实现）
    /// 
    /// 用于首次启动生成默认配置文件模板
    pub fn save_to_file<P: AsRef<Path>>(&self, _path: P) -> Result<(), std::io::Error> {
        // TODO: 实现配置序列化
        // 生成带注释的配置文件，说明每个字段的作用
        Ok(())
    }
}
