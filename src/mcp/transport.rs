// 传输层：stdio / streamable HTTP 的装配与优雅关停，版本校验与门卫。

use std::sync::Arc;

use axum::routing::Router;
use rmcp::transport::streamable_http_server::session::local::{LocalSessionManager, SessionConfig};
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::ctx::Ctx;
use crate::errors;
use crate::mcp::handler::{self, XyzServer};
use crate::mcp::{Options, protocol_version};
use crate::registry::Registry;

/// run 是 mcp 模式的入口：解析 transport 与 flags，构建服务器并服务直到
/// 传输结束（返回进程退出码）。
pub(crate) fn run(ctx: &Ctx, reg: &Registry, args: &[String], base: Options) -> i32 {
    let (transport, mut opts) = match crate::mcp::args::parse_args(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("mcp: {e}");
            eprintln!("{}", crate::lang::t("mcp.usage"));
            return 2;
        }
    };
    // 预设（--xyz.*）作为默认，命令行 flag 优先。
    opts.merge_defaults(&base);
    match transport.as_str() {
        "stdio" | "sse" | "http" => {}
        other => {
            eprintln!("mcp: unknown transport {other:?} (want stdio|http)");
            return 2;
        }
    }
    if transport == "sse" {
        eprintln!("mcp: {}", crate::lang::t("mcp.err_sse_removed"));
        return 2;
    }
    if let Err(e) = validate_versions(&opts.versions) {
        eprintln!("mcp: {e}");
        return 2;
    }
    let server_ctx = Arc::new(ctx.clone());
    let server = match handler::build(reg, &opts, Arc::clone(&server_ctx)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mcp: {e}");
            return 2;
        }
    };
    match transport.as_str() {
        "stdio" => {
            if !opts.bearer_tokens.is_empty() {
                crate::logx::warnf(format_args!("{}", crate::lang::t("warn.bearer_stdio")));
            }
            serve_stdio(&server)
        }
        _ => serve_http(&opts, &server),
    }
}

/// stdio 读取适配器：底层 EOF 时若最后一条消息没有换行终止，补一个 '\n'。
///
/// rmcp 的行读取路径把 EOF 时缓冲区里的残留字节当作「不完整的尾消息」丢弃
/// （`async_rw.rs` 的 `LineRead::Eof` 分支），因此客户端最后一条不带换行的
/// JSON-RPC 请求会永久无响应。这里在 EOF 处补一个换行让该行被正常投递；
/// 已经以换行结尾（或空流）的输入不会被多加内容。
struct EofNewline<R> {
    inner: R,
    last_was_newline: bool,
    done: bool,
}

impl<R> EofNewline<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            last_was_newline: true,
            done: false,
        }
    }
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for EofNewline<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.done {
            return std::task::Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        match std::pin::Pin::new(&mut this.inner).poll_read(cx, buf) {
            std::task::Poll::Ready(Ok(())) => {
                let filled = buf.filled().len();
                if filled > before {
                    this.last_was_newline = buf.filled()[filled - 1] == b'\n';
                } else {
                    // 底层 EOF：补齐缺失的换行（下一轮 poll 返回 EOF）。
                    this.done = true;
                    if !this.last_was_newline && buf.remaining() > 0 {
                        buf.put_slice(b"\n");
                    }
                }
                std::task::Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// stdio：进程内传输，客户端断开（EOF/关停）是正常退出。
fn serve_stdio(server: &XyzServer) -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("mcp: {e}");
            return 1;
        }
    };
    rt.block_on(async move {
        let ct = tokio_util::sync::CancellationToken::new();
        let server = server.clone();
        // ctx 取消 → 传输关停（等价 Go 的 signal ctx 贯穿）。
        let watcher = {
            let ct = ct.clone();
            let ctx = Arc::clone(&server.ctx);
            tokio::spawn(async move {
                ctx.cancelled_async().await;
                ct.cancel();
            })
        };
        // 用与本库读取适配器组合的 AsyncRwTransport 取代裸 stdio()：EOF 前的
        // 未终止行也要被投递（见 EofNewline 的说明）。
        let transport = rmcp::transport::async_rw::AsyncRwTransport::new_server(
            EofNewline::new(tokio::io::stdin()),
            tokio::io::stdout(),
        );
        let res = rmcp::service::ServiceExt::serve_with_ct(server, transport, ct).await;
        watcher.abort();
        match res {
            Ok(running) => {
                let _ = running.waiting().await;
                0
            }
            Err(_e) => {
                // 客户端断开是正常退出；协议初始化失败打印后退出 1。
                0
            }
        }
    })
}

/// streamable HTTP：tower 服务套 axum（官方示例同款），Bearer/CORS 复用
/// httpapi 积木。
fn serve_http(opts: &Options, server: &XyzServer) -> i32 {
    let router = match streamable_router(opts, server.clone()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("mcp: {e}");
            return 2;
        }
    };
    let mut router = router;
    // 中间件（由外到内）：CORS → Bearer → 路由。
    if !opts.bearer_tokens.is_empty() {
        let tokens: Arc<std::collections::HashSet<String>> =
            Arc::new(opts.bearer_tokens.iter().cloned().collect());
        router = router.layer(axum::middleware::from_fn(
            crate::httpapi::middleware::bearer_mw(tokens),
        ));
    }
    if !opts.cors_origins.is_empty() {
        let origins: Arc<std::collections::HashSet<String>> =
            Arc::new(opts.cors_origins.iter().cloned().collect());
        router = router.layer(axum::middleware::from_fn(
            crate::httpapi::middleware::cors_mw(origins),
        ));
    }
    crate::logx::debugf(format_args!(
        "streamable HTTP: session_timeout={:?} cors={} stateless={}",
        opts.session_timeout,
        opts.cors_origins.len(),
        opts.stateless
    ));
    let addr = if opts.addr.is_empty() {
        ":8080".to_string()
    } else {
        opts.addr.clone()
    };
    crate::logx::infof(format_args!(
        "{}",
        crate::lang::tf("log.mcp_listening", &[&addr])
    ));
    let ctx = Arc::clone(&server.ctx);
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("mcp: {e}");
            return 1;
        }
    };
    rt.block_on(async move {
        let listener =
            match tokio::net::TcpListener::bind(&crate::httpapi::normalize_addr(&addr)).await {
                Ok(l) => l,
                Err(e) => {
                    crate::logx::errorf(format_args!("{e}"));
                    return 1;
                }
            };
        let handle = axum_server::Handle::new();
        let serve_fut = {
            let router = router.clone();
            let handle = handle.clone();
            tokio::spawn(async move {
                let server = match axum_server::from_tcp(listener.into_std().unwrap()) {
                    Ok(s) => s,
                    Err(e) => {
                        crate::logx::errorf(format_args!("{e}"));
                        return 1;
                    }
                };
                let _ = server
                    .handle(handle)
                    .serve(router.into_make_service())
                    .await;
                0
            })
        };
        tokio::select! {
            r = serve_fut => r.unwrap_or(1),
            _ = async move {
                ctx.cancelled_async().await;
                handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
            } => {
                crate::logx::infof(format_args!("{}", crate::lang::t("log.graceful")));
                0
            }
        }
    })
}

/// 构建 streamable HTTP 的 axum 路由（可挂 /mcp 或独立 serve）。
pub(crate) fn streamable_router(opts: &Options, server: XyzServer) -> errors::Result<Router> {
    let service = streamable_service(opts, server)?;
    Ok(Router::new().fallback_service(service))
}

pub(crate) fn streamable_service(
    opts: &Options,
    server: XyzServer,
) -> errors::Result<StreamableHttpService<XyzServer, LocalSessionManager>> {
    let mut session_config = SessionConfig::default();
    session_config.keep_alive = if opts.session_timeout.is_zero() {
        None
    } else {
        Some(opts.session_timeout)
    };
    let mut manager = LocalSessionManager::default();
    manager.session_config = session_config;
    let mut config = StreamableHttpServerConfig::default();
    config.legacy_session_mode = !opts.stateless;
    config.json_response = opts.json_response;
    // rmcp 3.6.0 只在 stateless 路径实现 json_response；legacy session 始终
    // 回 SSE——明确告警，避免该选项看起来生效。
    if opts.json_response && !opts.stateless {
        crate::logx::warnf(format_args!(
            "--json-response has no effect without --stateless (rmcp's legacy session mode always answers text/event-stream)"
        ));
    }
    Ok(StreamableHttpService::new(
        move || Ok::<_, std::io::Error>(server.clone()),
        Arc::new(manager),
        config,
    ))
}

/// serve 模式 /mcp 挂载入口：不可用（例如未配置）时 None。
pub(crate) fn mountable_router(reg: &Registry, opts: &Options) -> errors::Result<Router> {
    let server = handler::build(
        reg,
        opts,
        opts.default_ctx
            .clone()
            .unwrap_or_else(|| Arc::new(Ctx::new())),
    )?;
    streamable_router(opts, server)
}

/// --versions 的注册期校验（子集 + 非空）。
pub fn validate_versions(versions: &[String]) -> errors::Result<()> {
    for v in versions {
        if v.is_empty() {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                "mcp: empty protocol version in --versions".to_string(),
            ));
        }
        if protocol_version(v).is_none() {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!(
                    "mcp: unknown protocol version {v:?} (known: {})",
                    crate::mcp::DEFAULT_VERSIONS.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

/// 显式版本列表与传输的能力交集检查（Go validateTransportVersions 对应物）。
/// Rust SDK 无 sse 传输；streamable HTTP 对 2026-07-28 需要 无状态
/// 模式——与 SDK 内部约束一致。
pub fn validate_transport_versions(transport: &str, opts: &Options) -> errors::Result<()> {
    if opts.versions.is_empty() {
        return Ok(()); // 默认全集：SDK 按传输自身能力裁剪
    }
    for v in &opts.versions {
        match transport {
            "http" => {
                if v != crate::mcp::PROTOCOL_V2026_07_28 || opts.stateless {
                    return Ok(());
                }
            }
            _ => return Ok(()), // stdio 全版本
        }
    }
    Err(errors::Error::new(
        errors::Kind::Internal,
        format!(
            "transport {transport:?} cannot serve any of the requested versions {:?}",
            opts.versions
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::EofNewline;
    use tokio::io::AsyncReadExt;

    async fn read_all(input: &[u8]) -> Vec<u8> {
        let mut r = EofNewline::new(input);
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn eof_without_newline_gets_one() {
        assert_eq!(read_all(b"{\"a\":1}").await, b"{\"a\":1}\n");
        assert_eq!(read_all(b"a\nb").await, b"a\nb\n");
    }

    #[tokio::test]
    async fn eof_with_newline_is_untouched() {
        assert_eq!(read_all(b"{\"a\":1}\n").await, b"{\"a\":1}\n");
        assert_eq!(read_all(b"").await, b"");
        assert_eq!(read_all(b"x\r\n").await, b"x\r\n");
    }
}
