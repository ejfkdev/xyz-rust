// httpapi 是 HTTP 前端，构建于 axum（Rust 生态的事实标准 HTTP 栈——
// 官方 rmcp 的 streamable HTTP 服务同栈，serve 模式把 /mcp 挂同一端口时
// 零桥接）。职责对齐 Go 版：
//
//   - HTTPHints.method + path 定义路由；path 模板 {name} 映射到绑定了
//     http="path" 的字段；
//   - 未标注位置（或标注 http:"query"）的字段默认从 query 绑定；
//     http:"header"（http_name）从请求头绑定；JSON body 合并为入参基底；
//   - 响应是裸 JSON（无信封），与 CLI --json 同形；错误经共享错误分类学
//     映射为 HTTP 状态码；
//   - GET /healthz 探活；GET /openapi.json 暴露与 MCP inputSchema 同源的
//     OpenAPI 3 文档。
//
// 没有 HTTP hints 的条目不路由。Go 版声称「纯标准库」——Rust std 无
// HTTP 服务器，这里的对等物是 axum（标注在 README 的差异节）。

pub mod middleware;
pub mod openapi;

#[cfg(test)]
mod httpapi_test;

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{Router, delete, get, head, options, patch, post, put};
use serde_json::{Map, Value};

use crate::config::Config;
use crate::ctx::Ctx;
use crate::errors;
use crate::registry::Registry;
use crate::spec::entry::Entry;

/// 请求体上限 1MiB（Go maxBodyBytes 同值）。
pub const MAX_BODY_BYTES: usize = 1 << 20;

/// Handler 构建 serve 模式的整表路由（REST + /healthz + /openapi.json）。
/// method+path 冲突是注册期错误（Go 用 recover 包 mux panic 转错误，
/// 这里显式前置检查，行为等价）。
pub fn router(reg: &Registry, ctx: Arc<Ctx>) -> errors::Result<Router> {
    router_with(reg, ctx, std::collections::HashMap::new())
}

/// 带通道级默认参数的路由器（serve --default k=v：缺席键补上）。
pub(crate) fn router_with(
    reg: &Registry,
    ctx: Arc<Ctx>,
    defaults: std::collections::HashMap<String, String>,
) -> errors::Result<Router> {
    let mut r: Router = Router::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for e in reg.all() {
        if e.http.skip {
            continue; // 通道层面整体移除
        }
        if e.http.method.is_empty() || e.http.path.is_empty() {
            continue; // 该命令没有声明 HTTP 路由（CLI/MCP 专用）
        }
        if !seen.insert((e.http.method.clone(), e.http.path.clone())) {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!(
                    "httpapi: route {:?} {:?} conflicts with an existing route",
                    e.http.method, e.http.path
                ),
            ));
        }
        let h = handle_entry_c(e.clone(), Arc::clone(&ctx), Arc::new(defaults.clone()));
        let mr = method_router(&e.http.method, h)?;
        r = r.route(&e.http.path, mr);
    }
    r = r
        .route("/healthz", get(healthz))
        .route("/openapi.json", get(openapi::openapi_handler(reg)));
    Ok(r)
}

fn method_router<H, T>(method: &str, h: H) -> errors::Result<axum::routing::MethodRouter>
where
    H: axum::handler::Handler<T, ()>,
    T: 'static,
{
    // axum 的 MethodRouter 覆盖常规动词；其余动词（Go net/http 的任意
    // 方法）在注册期给出清晰错误。
    Ok(match method {
        "GET" => get(h),
        "POST" => post(h),
        "PUT" => put(h),
        "PATCH" => patch(h),
        "DELETE" => delete(h),
        "HEAD" => head(h),
        "OPTIONS" => options(h),
        other => {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!("httpapi: unsupported HTTP method {other:?}"),
            ));
        }
    })
}

/// 单条命令的完整绑定处理器（query/path/header/body + 错误映射），无
/// 路由。挂到任意 axum Router 上（docs/adapters.md 有示例）。
/// 轮询 Handler：axum 闭包 handler 的请求抽取元组带私有 ViaRequest
/// 标记、无法具名，这里以自实现 Handler 的具名类型给出清晰公开 API
/// （axum 文档支持的自定义 handler 路线）。
#[derive(Clone)]
pub struct EntryHandler {
    pub(crate) entry: Arc<Entry>,
    pub(crate) ctx: Arc<Ctx>,
    pub(crate) defaults: Arc<std::collections::HashMap<String, String>>,
}

impl axum::handler::Handler<(), ()> for EntryHandler {
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;
    fn call(self, req: Request, _state: ()) -> Self::Future {
        Box::pin(async move { handle_request(&self.entry, &self.ctx, &self.defaults, req).await })
    }
}

pub fn handler_for(entry: Arc<Entry>) -> EntryHandler {
    handle_entry_c(
        entry,
        Arc::new(Ctx::new()),
        Arc::new(std::collections::HashMap::new()),
    )
}

/// 入口处理器构建（serve 模式注入取消上下文）。
fn handle_entry_c(
    entry: Arc<Entry>,
    ctx: Arc<Ctx>,
    defaults: Arc<std::collections::HashMap<String, String>>,
) -> EntryHandler {
    EntryHandler {
        entry,
        ctx,
        defaults,
    }
}

async fn handle_request(
    entry: &Arc<Entry>,
    ctx: &Arc<Ctx>,
    defaults: &std::collections::HashMap<String, String>,
    req: Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let method = parts.method.clone();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().unwrap_or_default().to_string();
    let headers = parts.headers.clone();

    let mut m: Map<String, Value> = Map::new();
    // 铺底：HTTP 专属默认值（全局默认由 Invoke 补齐）。
    for (k, v) in entry.http_defaults() {
        m.insert(k, v);
    }
    // 通道级默认参数（serve --default k=v）：只补缺席键。
    for (k, v) in defaults.iter() {
        if !m.contains_key(k) {
            m.insert(k.clone(), Value::String(v.clone()));
        }
    }
    // JSON body 作为基础入参（非 GET/HEAD 且带体时解析）。
    let body_bytes: Vec<u8> = if method != Method::GET && method != Method::HEAD {
        collect_body(body, MAX_BODY_BYTES).await
    } else {
        Vec::new()
    };
    if !body_bytes.is_empty() {
        let json_declared = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("application/json"))
            .unwrap_or(false);
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(Value::Object(parsed)) => {
                for (k, v) in parsed {
                    m.insert(k, v);
                }
            }
            Ok(_) => {
                return write_error(StatusCode::BAD_REQUEST, "invalid JSON body");
            }
            Err(_) if json_declared => {
                // 显式声明 application/json 却解析失败：严格 400。
                return write_error(StatusCode::BAD_REQUEST, "invalid JSON body");
            }
            Err(_) => {
                // 非 JSON 声明（如表单体）解析失败：交给 form 绑定，不提前判死。
            }
        }
    }
    // form 字段惰性解析：只在该条目确实有 http:"form" 字段时解析表单。
    let mut form_parsed: Option<Vec<(String, String)>> = None;

    for f in &entry.root.children {
        if f.skip {
            // json:"-" 的注入字段：header 值以 Rust 字段名为键送达。
            if f.http.location == "header"
                && let Some(v) = headers.get(http_name(f)).and_then(|v| v.to_str().ok())
                && !v.is_empty()
            {
                m.insert(f.name.clone(), Value::String(v.to_string()));
            }
            continue;
        }
        match f.http.location.as_str() {
            "" | "query" => {
                if let Some(vs) = query_values(&query, &f.json_name) {
                    if is_string_slice(f) {
                        m.insert(
                            f.json_name.clone(),
                            Value::Array(vs.iter().map(|s| Value::String(s.clone())).collect()),
                        );
                    } else if let Some(first) = vs.first() {
                        m.insert(f.json_name.clone(), Value::String(first.clone()));
                    }
                }
            }
            "header" => {
                if let Some(v) = headers.get(http_name(f)).and_then(|v| v.to_str().ok())
                    && !v.is_empty()
                {
                    m.insert(f.json_name.clone(), Value::String(v.to_string()));
                }
            }
            "path" => {
                if let Some(v) = path_param(&entry.http.path, &path, http_name(f)) {
                    m.insert(f.json_name.clone(), Value::String(v));
                }
            }
            "form" => {
                if form_parsed.is_none() {
                    // 与上游一致：form 值仅来自请求体（已读字节，避免二读 body）。
                    form_parsed = Some(crate::httpapi::parse_form_query(&String::from_utf8_lossy(
                        &body_bytes,
                    )));
                }
                if let Some((_, v)) = form_parsed
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|(k, _)| k == &f.json_name)
                {
                    m.insert(f.json_name.clone(), Value::String(v.clone()));
                }
            }
            _ => {}
        }
    }

    match (entry.invoke)(ctx, &m) {
        Ok(out) => {
            let s = serde_json::to_string_pretty(&out).unwrap_or_else(|_| "null".to_string());
            json_response(StatusCode::OK, &s)
        }
        Err(e) => {
            let kind = errors::classify(&e).unwrap_or(errors::Kind::Internal);
            write_error(
                StatusCode::from_u16(errors::http_status(kind))
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                &cause_message(&e),
            )
        }
    }
}

/// httpName 计算线上名：http_name 覆盖 > JSON 名 > Rust 字段名。
pub(crate) fn http_name(f: &crate::spec::FieldMeta) -> &str {
    if !f.http.name.is_empty() {
        return f.http.name.as_str();
    }
    if !f.json_name.is_empty() {
        return f.json_name.as_str();
    }
    f.name.as_str()
}

fn is_string_slice(f: &crate::spec::FieldMeta) -> bool {
    matches!(f.kind, crate::spec::FieldKind::Slice)
}

/// 手写 query/form 解析（零第三方依赖的 urlencoded 子集）：按 & 拆对、
/// % 解码。
pub(crate) fn parse_form_query(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (pct_decode(k), pct_decode(v)),
            None => (pct_decode(pair), String::new()),
        })
        .collect()
}

fn query_values(query: &str, key: &str) -> Option<Vec<String>> {
    let pairs = parse_form_query(query);
    let hit: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .collect();
    if hit.is_empty() { None } else { Some(hit) }
}

/// 按模板段摘出 {name} 路径参数（路由已由 axum 匹配，这里只取值）。
fn path_param(template: &str, path: &str, name: &str) -> Option<String> {
    let tsegs: Vec<&str> = template.split('/').collect();
    let psegs: Vec<&str> = path.split('/').collect();
    if tsegs.len() != psegs.len() {
        return None;
    }
    for (t, p) in tsegs.iter().zip(psegs.iter()) {
        if let Some(inner) = t.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
            && inner == name
        {
            return Some(pct_decode(p));
        }
    }
    None
}

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 1 {
            let h = hex_val(bytes.get(i + 1));
            let l = hex_val(bytes.get(i + 2));
            if let (Some(h), Some(l)) = (h, l) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: Option<&u8>) -> Option<u8> {
    match b.copied()? {
        b'0'..=b'9' => Some(b.unwrap() - b'0'),
        b'a'..=b'f' => Some(b.unwrap() - b'a' + 10),
        b'A'..=b'F' => Some(b.unwrap() - b'A' + 10),
        _ => None,
    }
}

fn cause_message(e: &errors::Error) -> String {
    match e.cause() {
        Some(c) => c.to_string(),
        None => e.to_string(),
    }
}

fn write_error(status: StatusCode, msg: &str) -> Response {
    let msg = if msg.is_empty() {
        status.canonical_reason().unwrap_or("error").to_string()
    } else {
        msg.to_string()
    };
    // Go writeError：紧凑 JSON（结果才走 pretty）。
    let body = serde_json::to_string(&serde_json::json!({ "error": msg }))
        .unwrap_or_else(|_| "{\"error\":\"\"}".to_string());
    json_response(status, &body)
}

fn json_response(status: StatusCode, body: &str) -> Response {
    let mut b = body.to_string();
    b.push('\n');
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        b,
    )
        .into_response()
}

async fn healthz() -> Response {
    json_response(StatusCode::OK, "{\"status\":\"ok\"}")
}

/// 逐块收集 body 直到 cap 字节（Go 的 LimitReader + ReadAll 截断语义）。
async fn collect_body(body: axum::body::Body, cap: usize) -> Vec<u8> {
    use futures_util::StreamExt as _;
    let mut out = Vec::new();
    let mut stream = body.into_data_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        let rem = cap.saturating_sub(out.len());
        if rem == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..chunk.len().min(rem)]);
    }
    out
}

/// serve 模式：解析裸名 flag、装配路由与中间件、TLS/优雅关停。
pub(crate) fn serve(ctx: &Ctx, reg: &Registry, args: &[String], cfg: Config) -> i32 {
    let cfg = crate::builtins::parse_serve_args(args, cfg);
    let sctx = Arc::new(ctx.clone());
    let mut router = match router_with(reg, Arc::clone(&sctx), cfg.channel_defaults.clone()) {
        Ok(r) => r,
        Err(e) => {
            crate::logx::errorf(format_args!("{e}"));
            return 2;
        }
    };
    let mut mcp_note = String::new();
    #[cfg(feature = "mcp")]
    {
        if let Some(m) = crate::mcp::mountable(
            reg,
            &crate::mcp::Options {
                bearer_tokens: cfg.bearer_tokens.clone(),
                default_ctx: Some(sctx.clone()),
                defaults: cfg.channel_defaults.clone(),
                ..Default::default()
            },
        ) {
            router = router.nest_service("/mcp", m);
            mcp_note = " + /mcp".to_string();
        }
    }
    if !cfg.cors_origins.is_empty() {
        crate::logx::debugf(format_args!(
            "{}",
            crate::lang::tf("log.cors_on", &[&format!("{:?}", cfg.cors_origins)])
        ));
    }
    // 中间件链（由外到内）：CORS 预检（鉴权前，浏览器预检不带凭据）→
    // Bearer → Gzip → 路由。顺序语义在 middleware.rs 集中组装并有测试锁定。
    let router = middleware::apply(
        router,
        cfg.bearer_tokens.clone(),
        cfg.cors_origins.clone(),
        cfg.timeout,
    );

    let tls_on = !cfg.cert_file.is_empty() || !cfg.key_file.is_empty();
    if tls_on && (cfg.cert_file.is_empty() || cfg.key_file.is_empty()) {
        crate::logx::errorf(format_args!("TLS requires both --tls-cert and --tls-key"));
        return 2;
    }
    let scheme = if tls_on { "https" } else { "http" };
    crate::logx::infof(format_args!(
        "{}",
        crate::lang::tf("log.serve_listening", &[scheme, &cfg.addr, &mcp_note])
    ));

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            crate::logx::errorf(format_args!("{e}"));
            return 1;
        }
    };
    rt.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(&cfg.addr).await {
            Ok(l) => l,
            Err(e) => {
                crate::logx::errorf(format_args!("{e}"));
                return 1;
            }
        };
        let handle = axum_server::Handle::new();
        let cert_file = cfg.cert_file.clone();
        let key_file = cfg.key_file.clone();
        let serve = {
            let router = router.clone();
            let handle = handle.clone();
            move || async move {
                if tls_on {
                    let tls_config = match load_rustls_config(&cert_file, &key_file) {
                        Ok(c) => c,
                        Err(e) => {
                            crate::logx::errorf(format_args!("{e}"));
                            return 1;
                        }
                    };
                    let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(
                        std::sync::Arc::new(tls_config),
                    );
                    let _ = axum_server::from_tcp_rustls(listener.into_std().unwrap(), rustls_cfg)
                        .handle(handle.clone())
                        .serve(router.clone().into_make_service())
                        .await;
                } else {
                    let _ = axum_server::from_tcp(listener.into_std().unwrap())
                        .handle(handle.clone())
                        .serve(router.clone().into_make_service())
                        .await;
                };
                0
            }
        };
        let _ = &serve;
        let server_handle = handle.clone();
        let serve_fut = tokio::spawn(async move { serve().await });
        let cancel_fut = async move {
            sctx.cancelled_async().await;
            server_handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
        };
        tokio::select! {
            join_res = serve_fut => join_res.unwrap_or(1),
            _ = cancel_fut => {
                crate::logx::infof(format_args!("{}", crate::lang::t("log.graceful")));
                0
            }
        }
    })
}

#[cfg(feature = "http-stack")]
fn load_rustls_config(cert_file: &str, key_file: &str) -> errors::Result<rustls::ServerConfig> {
    use std::io::BufReader;
    let certs = rustls_pemfile::certs(&mut BufReader::new(
        std::fs::File::open(cert_file).map_err(errors::Error::from)?,
    ))
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| errors::Error::new(errors::Kind::Internal, format!("cert parse: {e}")))?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(
        std::fs::File::open(key_file).map_err(errors::Error::from)?,
    ))
    .map_err(|e| errors::Error::new(errors::Kind::Internal, format!("key parse: {e}")))?
    .ok_or_else(|| {
        errors::Error::new(errors::Kind::Internal, "no private key found".to_string())
    })?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| errors::Error::new(errors::Kind::Internal, format!("tls config: {e}")))?;
    Ok(config)
}
