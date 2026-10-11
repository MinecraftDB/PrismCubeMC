//! 协议分发器：Prism 单端口 19132 的多协议识别与路由
//! 
//! 【核心设计】
//! 一个端口同时处理：
//! - TCP：Java Edition 连接、NetherNet WebSocket 信令
//! - UDP：基岩版 RakNet、NetherNet DTLS 数据通道
//! 
//! 通过首字节特征码识别协议类型，分发到对应处理器。

use std::net::SocketAddr;
use tokio::net::{TcpListener, UdpSocket};
use tracing::{debug, error, info, warn};

/// RakNet 协议常量：用于 UDP 包识别
mod raknet_consts {
    /// Unconnected Ping：客户端探测服务器
    pub const UNCONNECTED_PING: u8 = 0x01;
    /// Unconnected Pong：服务器响应探测
    pub const UNCONNECTED_PONG: u8 = 0x1C;
    /// Connected Ping：已连接会话的心跳
    pub const CONNECTED_PING: u8 = 0x00;
    /// 连接请求第一阶段
    pub const OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
    /// 连接请求第二阶段
    pub const OPEN_CONNECTION_REQUEST_2: u8 = 0x07;
}

/// NetherNet / WebRTC 协议常量
mod nethernet_consts {
    /// DTLS 握手起始：ContentType = handshake(22)
    pub const DTLS_HANDSHAKE: u8 = 0x16;
}

/// 协议分发器
/// 
/// 【字段说明】
/// - port: 监听的端口号，TCP 和 UDP 共用
#[derive(Debug)]
pub struct Dispatcher {
    /// 监听端口：Prism 单端口汇聚点
    port: u16,
}

impl Dispatcher {
    /// 创建新的分发器
    /// 
    /// 【参数】
    /// - port: 监听端口，从 ServerConfig.server_port 传入
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// 启动分发循环
    /// 
    /// 【流程】
    /// 1. 绑定 TCP 和 UDP 的同一端口
    /// 2. 使用 tokio::select! 并发处理两种协议
    /// 3. 根据首字节特征码识别协议，分发到对应处理器
    pub async fn start(&self) -> std::io::Result<()> {
        let port = self.port;
        let bind_addr = format!("0.0.0.0:{}", port);
        
        info!("正在绑定 Prism 汇聚端口: {}", bind_addr);

        // 绑定 TCP：用于 Java Edition 和 NetherNet WebSocket 信令
        let tcp_listener = TcpListener::bind(&bind_addr).await?;
        let actual_tcp = tcp_listener.local_addr()?;
        info!("TCP 监听已启动: {}", actual_tcp);

        // 绑定 UDP：用于基岩版 RakNet 和 NetherNet DTLS
        let udp_socket = UdpSocket::bind(&bind_addr).await?;
        let actual_udp = udp_socket.local_addr()?;
        info!("UDP 监听已启动: {}", actual_udp);

        info!("Prism 分发器运行中，等待连接...");

        // 主循环：并发处理 TCP 和 UDP
        loop {
            tokio::select! {
                // TCP 分支：Java Edition / NetherNet 信令
                accept_result = tcp_listener.accept() => {
                    match accept_result {
                        Ok((stream, addr)) => {
                            self.handle_tcp_connection(stream, addr).await;
                        }
                        Err(e) => {
                            error!(error = %e, "TCP 接受连接失败");
                        }
                    }
                }

                // UDP 分支：RakNet / NetherNet 数据
                recv_result = Self::recv_udp_packet(&udp_socket) => {
                    match recv_result {
                        Ok((data, addr)) => {
                            self.handle_udp_packet(data, addr).await;
                        }
                        Err(e) => {
                            error!(error = %e, "UDP 接收数据包失败");
                        }
                    }
                }
            }
        }
    }

    /// 接收单个 UDP 数据包
    /// 
    /// 【缓冲区】
    /// 2048 字节：RakNet 最大帧约 1500 字节（MTU），
    /// NetherNet / WebRTC 数据包通常更小
    async fn recv_udp_packet(
        socket: &UdpSocket,
    ) -> std::io::Result<(bytes::Bytes, SocketAddr)> {
        let mut buf = vec![0u8; 2048];
        let (len, addr) = socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        Ok((bytes::Bytes::from(buf), addr))
    }

    /// 处理 UDP 数据包：协议识别与路由
    /// 
    /// 【识别逻辑】
    /// 首字节：
    ///   0x01, 0x05, 0x07, 0x1C → RakNet（基岩版传统协议）
    ///   0x16 → DTLS（NetherNet）
    ///   其他 → 已建立的会话数据，按 session 分发
    async fn handle_udp_packet(&self, data: bytes::Bytes, addr: SocketAddr) {
        if data.is_empty() {
            warn!(%addr, "收到空 UDP 包，忽略");
            return;
        }

        let first_byte = data[0];
        debug!(%addr, first_byte = format!("0x{:02X}", first_byte), "收到 UDP 包");

        match first_byte {
            // RakNet 协议族
            raknet_consts::UNCONNECTED_PING
            | raknet_consts::UNCONNECTED_PONG
            | raknet_consts::CONNECTED_PING
            | raknet_consts::OPEN_CONNECTION_REQUEST_1
            | raknet_consts::OPEN_CONNECTION_REQUEST_2 => {
                debug!(%addr, "识别为 RakNet 协议");
                self.handle_raknet(data, addr).await;
            }

            // NetherNet / DTLS
            nethernet_consts::DTLS_HANDSHAKE => {
                debug!(%addr, "识别为 NetherNet/DTLS 协议");
                self.handle_nethernet(data, addr).await;
            }

            // 已建立的会话或未知协议
            _ => {
                warn!(%addr, first_byte = format!("0x{:02X}", first_byte), 
                      "无法识别的 UDP 包");
            }
        }
    }

    /// 处理 TCP 新连接
    /// 
    /// 【协议识别】
    /// 首字节/首包特征：
    ///   0xFE → 旧版 Minecraft ping（1.4-1.6）
    ///   0x00 + VarInt 长度 → 现代 Java Edition 握手
    ///   HTTP 请求 → WebSocket / NetherNet 信令
    async fn handle_tcp_connection(
        &self,
        stream: tokio::net::TcpStream,
        addr: SocketAddr,
    ) {
        info!(%addr, "收到 TCP 连接");

        // 尝试读取首字节进行协议识别
        let mut peek_buf = [0u8; 16];
        
        match stream.peek(&mut peek_buf).await {
            Ok(0) => {
                debug!(%addr, "TCP 连接立即关闭");
                return;
            }
            Ok(n) => {
                debug!(%addr, bytes_peeked = n, "TCP 首字节识别");

                if peek_buf[0] == 0xFE {
                    // 旧版 Minecraft ping（1.4-1.6）
                    info!(%addr, "识别为旧版 Java ping");
                    self.handle_java_legacy_ping(stream, addr).await;
                } else if peek_buf[0] == b'G' || peek_buf[0] == b'P' {
                    // HTTP 请求（GET / POST）→ NetherNet WebSocket 信令
                    info!(%addr, "识别为 HTTP/WebSocket 信令");
                    self.handle_nethernet_signal(stream, addr).await;
                } else {
                    // 现代 Java Edition（长度前缀 VarInt）
                    info!(%addr, "识别为现代 Java 连接");
                    self.handle_java(stream, addr).await;
                }
            }
            Err(e) => {
                error!(%addr, error = %e, "TCP peek 失败");
            }
        }
    }

    // ═══════════════════════════════════════════════════════════
    // 协议处理器骨架（后续实现）
    // ═══════════════════════════════════════════════════════════

    /// 处理 RakNet 协议数据包（基岩版 0.14 - 1.20.x）
    async fn handle_raknet(&self, _data: bytes::Bytes, _addr: SocketAddr) {
        // TODO: 创建或获取 RakNet 会话
        // TODO: 处理未连接阶段的 Ping/Pong、连接请求
        // TODO: 处理已连接阶段的帧分割、可靠性、有序性
        debug!("RakNet 处理器占位");
    }

    /// 处理 NetherNet 协议数据包（基岩版 26.3+）
    async fn handle_nethernet(&self, _data: bytes::Bytes, _addr: SocketAddr) {
        // TODO: 处理 DTLS 握手状态机
        // TODO: 管理 ICE 候选（本地、反射、中继地址）
        // TODO: 建立 SCTP/QUIC 流后，传输游戏数据包
        debug!("NetherNet 处理器占位");
    }

    /// 处理现代 Java Edition 连接
    async fn handle_java(&self, _stream: tokio::net::TcpStream, _addr: SocketAddr) {
        // TODO: 创建 Java 会话，包装 stream 为 Framed
        // TODO: 读取 Handshake 包，提取协议版本
        // TODO: 根据协议版本创建对应版本的 PacketCodec
        // TODO: 进入状态机（Handshake → Status/Login → Play）
        info!("Java 处理器占位");
    }

    /// 处理旧版 Java ping（1.4.2 - 1.6.4）
    async fn handle_java_legacy_ping(
        &self,
        _stream: tokio::net::TcpStream,
        _addr: SocketAddr,
    ) {
        // TODO: 读取 0xFE 0x01 请求
        // TODO: 构造旧版响应（MOTD、在线人数）
        // TODO: 发送 0xFF 开头的 UTF-16BE 字符串
        info!("旧版 Java ping 处理器占位");
    }

    /// 处理 NetherNet TCP 信令（WebSocket）
    async fn handle_nethernet_signal(
        &self,
        _stream: tokio::net::TcpStream,
        _addr: SocketAddr,
    ) {
        // TODO: 处理 HTTP WebSocket 握手
        // TODO: 解析 JSON 信令（ICE candidate, SDP offer/answer）
        // TODO: 协调 UDP 端的 DTLS 握手
        info!("NetherNet 信令处理器占位");
    }
}
