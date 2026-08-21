// mcp 是 MCP 前端，构建于官方 Rust SDK（rmcp）。每条注册命令成为一个
// MCP 工具，经 stdio 与 streamable HTTP 两种传输服务，并支持钉定协议
// 版本子集。
//
// 输出契约：每次成功调用返回人类可读文本（与 CLI 前端同款渲染）到
// content，外加结构化 JSON 到 structuredContent；失败以 isError=true
// 结果携带分类错误消息。

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::ctx::Ctx;
use crate::registry::Registry;

pub mod args;
pub mod handler;
pub mod transport;

#[cfg(test)]
mod mcp_test;

/// 官方 Rust SDK 已知的协议版本（rmcp::ProtocolVersion 常量的字符串形态；
/// 与 Go SDK 同一集合，从 2024-11-05 到 2026-07-28）。
pub const PROTOCOL_V2024_11_05: &str = "2024-11-05";
pub const PROTOCOL_V2025_03_26: &str = "2025-03-26";
pub const PROTOCOL_V2025_06_18: &str = "2025-06-18";
pub const PROTOCOL_V2025_11_25: &str = "2025-11-25";
pub const PROTOCOL_V2026_07_28: &str = "2026-07-28"; // latest revision

/// 全集（协商偏好序：最新在前，对齐 Go DefaultVersions）。
pub const DEFAULT_VERSIONS: &[&str] = &[
    PROTOCOL_V2026_07_28,
    PROTOCOL_V2025_11_25,
    PROTOCOL_V2025_06_18,
    PROTOCOL_V2025_03_26,
    PROTOCOL_V2024_11_05,
];

/// Options 配置 MCP 前端。零值服务 SDK 已知的全部协议版本。
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// 服务器实现名与版本。默认：二进制名与 "0.0.0"。
    pub name: String,
    pub version: String,

    /// 钉定协议版本子集（DEFAULT_VERSIONS 的子集）；顺序即协商偏好序。
    /// 空 = 全集。
    pub versions: Vec<String>,

    /// 初始化后展示给客户端的说明。
    pub instructions: String,

    /// sse/http 传输的监听地址。默认 ":8080"。
    pub addr: String,

    /// streamable HTTP 以 application/json 而非 text/event-stream 应答
    /// （调试友好）。
    pub json_response: bool,

    /// 开启 streamable HTTP 无状态模式（SEP-2567）。
    pub stateless: bool,

    /// http 传输的 Bearer 校验（stdio 是本地进程，不受影响）。空 = 不校验。
    pub bearer_tokens: Vec<String>,

    /// streamable HTTP 空闲会话过期。
    pub session_timeout: Duration,

    /// http 传输的 CORS 白名单。
    pub cors_origins: Vec<String>,

    /// serve 模式 /mcp 挂载时注入的取消上下文（内部使用）。
    #[doc(hidden)]
    pub default_ctx: Option<Arc<Ctx>>,
}

/// 构建服务器实现：每条注册命令一个工具，inputSchema 直接来自注册表的
/// JSON Schema 生成；共享 Invoke 管线做全部解码与校验。
pub fn server(
    reg: &Registry,
    opts: Options,
    ctx: Arc<Ctx>,
) -> crate::errors::Result<handler::XyzServer> {
    handler::build(reg, &opts, ctx)
}

/// 根派发器入口：解析 mcp 模式的参数并派发传输（等价 Go runWithOptions，
/// cfg 注入 --xyz.* 折叠后的 preset）。
pub fn run_with_config(ctx: &Ctx, reg: &Registry, args: &[String], cfg: Config) -> i32 {
    let base = Options {
        addr: cfg.addr.clone(),
        bearer_tokens: cfg.bearer_tokens.clone(),
        ..Default::default()
    };
    transport::run(ctx, reg, args, base)
}

/// mcp 模式（默认 Options）。
pub fn run_context(ctx: &Ctx, reg: &Registry, args: &[String]) -> i32 {
    transport::run(ctx, reg, args, Options::default())
}

/// 供 serve 模式 /mcp 挂载的 streamable HTTP 路由；不可用时 None。
pub fn mountable(reg: &Registry, opts: &Options) -> Option<axum::routing::Router> {
    transport::mountable_router(reg, opts).ok()
}

/// 官方 SDK 的协议版本常量 ↔ 字符串（传输层转换用）。
pub(crate) fn protocol_version(s: &str) -> Option<rmcp::model::ProtocolVersion> {
    match s {
        PROTOCOL_V2024_11_05 => Some(rmcp::model::ProtocolVersion::V_2024_11_05),
        PROTOCOL_V2025_03_26 => Some(rmcp::model::ProtocolVersion::V_2025_03_26),
        PROTOCOL_V2025_06_18 => Some(rmcp::model::ProtocolVersion::V_2025_06_18),
        PROTOCOL_V2025_11_25 => Some(rmcp::model::ProtocolVersion::V_2025_11_25),
        PROTOCOL_V2026_07_28 => Some(rmcp::model::ProtocolVersion::V_2026_07_28),
        _ => None,
    }
}
