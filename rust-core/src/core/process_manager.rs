//! 通用进程管理器：外部子进程的生命周期管理
//! 
//! 【设计原则】
//! 1. 劫持启动：spawn_process 强制注入 PRISM_SESSION_TOKEN 环境变量
//! 2. 状态注入：通过 IPC 通道向子进程发送指令
//! 3. 优雅关闭：先协作请求，超时后 OS 信号，最后强杀兜底
//! 
//! 【红线】
//! 主进程退出前必须等待所有子进程安全退出，绝不允许孤儿进程残留。

use tokio::process::Child;
use tokio::time::{timeout, Duration};
use tracing::{error, info, warn};

/// 受管理的子进程
/// 
/// 【字段说明】
/// - name: 人类可读标识，用于日志
/// - child: tokio 子进程句柄，用于 wait/kill
#[derive(Debug)]
pub struct ManagedProcess {
    /// 进程可读名称
    pub name: String,
    
    /// 实际的子进程句柄
    /// 
    /// 使用 Option，因为等待退出时需要 take() 所有权
    pub child: Option<Child>,
}

impl ManagedProcess {
    /// 创建新的受管理进程
    pub fn new(name: impl Into<String>, child: Child) -> Self {
        Self {
            name: name.into(),
            child: Some(child),
        }
    }

    /// 获取进程 ID
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|c| c.id())
    }

    /// 等待进程退出，带超时
    pub async fn wait_for_exit(
        &mut self,
        timeout_duration: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let Some(mut child) = self.child.take() else {
            return Ok(None);
        };

        match timeout(timeout_duration, child.wait()).await {
            Ok(Ok(status)) => {
                info!(process = %self.name, code = ?status.code(), "子进程已退出");
                Ok(Some(status))
            }
            Ok(Err(e)) => {
                error!(process = %self.name, error = %e, "等待退出出错");
                self.child = Some(child);
                Err(e)
            }
            Err(_) => {
                warn!(process = %self.name, "等待子进程退出超时");
                self.child = Some(child);
                Ok(None)
            }
        }
    }

    /// 强制终止进程（SIGKILL / TerminateProcess）
    pub async fn force_kill(&mut self) -> std::io::Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };

        warn!(process = %self.name, pid = ?self.id(), "强制终止子进程");
        child.kill().await
    }
}

/// 进程管理器：统一管理所有外部子进程
/// 
/// 【核心职责】
/// 1. 劫持启动：spawn_process 强制注入环境变量
/// 2. 状态注入：send_command_to_process 动态控制
/// 3. 优雅关闭：shutdown_all 确保有序退出
#[derive(Debug, Default)]
pub struct ProcessManager {
    /// 所有受管理的子进程
    processes: Vec<ManagedProcess>,
    
    /// 会话签证：注入到所有子进程环境变量中
    /// 
    /// 子进程通过 IPC 连接回来时，必须出示此 Token 进行认证
    session_token: String,
}

impl ProcessManager {
    /// 创建新的进程管理器
    /// 
    /// 【参数】
    /// - session_token: 当前服务器运行的签证，从 LaunchManager 传入
    pub fn new(session_token: impl Into<String>) -> Self {
        Self {
            processes: Vec::new(),
            session_token: session_token.into(),
        }
    }

    /// 启动受管理子进程（劫持启动）
    /// 
    /// 【劫持的含义】
    /// 无论调用者想启动什么程序，我们都：
    /// 1. 将 PRISM_SESSION_TOKEN 注入环境变量
    /// 2. 启动子进程
    /// 3. 子进程后续通过 IPC 连接回来时，必须出示此 Token
    /// 
    /// 【参数】
    /// - name: 进程标识，如 "java-proxy"
    /// - command: 已配置好的 tokio::process::Command
    pub async fn spawn_process(
        &mut self,
        name: impl Into<String>,
        command: &mut tokio::process::Command,
    ) -> Result<&mut ManagedProcess, std::io::Error> {
        let name = name.into();
        
        // 步骤 1：注入会话签证到环境变量
        // 这是安全机制：防止非法进程冒充子进程连接 IPC
        command.env("PRISM_SESSION_TOKEN", &self.session_token);
        info!(%name, token = %self.session_token, "注入 PRISM_SESSION_TOKEN");

        // 步骤 2：启动子进程
        let child = command.spawn()?;
        let pid = child.id();
        info!(%name, ?pid, "子进程已启动");

        // 步骤 3：创建 ManagedProcess，加入管理列表
        let process = ManagedProcess::new(name, child);
        self.processes.push(process);

        // 返回可变引用
        self.processes.last_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "进程刚插入却找不到")
        })
    }

    /// 获取指定名称的进程可变引用
    pub fn get_process_mut(&mut self, name: &str) -> Option<&mut ManagedProcess> {
        self.processes.iter_mut().find(|p| p.name == name)
    }

    /// 获取指定名称的进程不可变引用
    pub fn get_process(&self, name: &str) -> Option<&ManagedProcess> {
        self.processes.iter().find(|p| p.name == name)
    }

    /// 向指定子进程发送指令（状态注入骨架）
    /// 
    /// 【设计意图】
    /// 通过 IPC 通道向子进程发送 JSON-RPC 指令，
    /// 例如动态切换版本上下文、热重载插件等。
    /// 
    /// 【参数】
    /// - process_name: 目标进程名称
    /// - method: 指令方法名
    /// - params: 可选参数，会被序列化为 JSON
    pub async fn send_command_to_process(
        &mut self,
        process_name: &str,
        method: &str,
        params: Option<String>,
    ) -> Result<(), String> {
        let _process = self.get_process(process_name)
            .ok_or_else(|| format!("进程 {} 不存在", process_name))?;
        
        // TODO: 建立 IPC 连接（TCP/Unix Socket）
        // TODO: 序列化 JSON-RPC 请求
        // TODO: 发送请求并等待响应
        
        info!(process = %process_name, %method, ?params, "发送指令（骨架）");
        
        // 骨架阶段：仅记录日志，不实际发送
        Ok(())
    }

    /// 优雅关闭所有子进程
    /// 
    /// 【关闭策略】
    /// 对每个进程（逆序，处理依赖关系）：
    ///   1. 尝试 OS 信号（SIGTERM / CTRL_BREAK_EVENT）
    ///   2. 等待退出（最多 timeout_secs）
    ///   3. 超时则强制终止（SIGKILL）
    /// 
    /// 【参数】
    /// - timeout_secs: 每个进程的总超时时间（秒）
    pub async fn shutdown_all(&mut self, timeout_secs: u64) {
        let total_timeout = Duration::from_secs(timeout_secs);
        // 信号等待占 80%，kill 缓冲占 20%
        let signal_timeout = Duration::from_secs((timeout_secs * 8) / 10);
        
        info!(
            process_count = self.processes.len(),
            timeout_secs,
            "开始优雅关闭所有子进程"
        );

        // 逆序关闭：后启动的先关闭（处理依赖关系）
        for i in (0..self.processes.len()).rev() {
            let process = &mut self.processes[i];
            let name = process.name.clone();
            
            info!(%name, "开始关闭子进程");

            // 步骤 1：发送 OS 关闭信号
            if let Some(child) = process.child.as_mut() {
                info!(%name, pid = ?process.id(), "发送 OS 关闭信号");
                let _ = child.start_kill();
            }

            // 步骤 2：等待进程自行退出
            match process.wait_for_exit(signal_timeout).await {
                Ok(Some(_)) => {
                    info!(%name, "子进程已优雅退出");
                    continue;
                }
                Ok(None) => {
                    warn!(%name, "OS 信号超时");
                }
                Err(e) => {
                    error!(%name, error = %e, "等待退出出错");
                }
            }

            // 步骤 3：强制终止（最后手段）
            warn!(%name, "执行强制终止");
            if let Err(e) = process.force_kill().await {
                error!(%name, error = %e, "强制终止失败");
            }
        }

        info!("所有子进程处理完成");
        self.processes.clear();
    }

    pub fn len(&self) -> usize {
        self.processes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }
}
