// 中间件积木（Go middleware.go 对应物）：Bearer 校验、CORS 白名单、
// Gzip 压缩。各自独立可组合（docs/adapters.md 档位 C）。
//
// 层顺序语义（tower：后 add 的层在外侧、先触发，等价 Go 的由外到内）：
//   CORS（预检在鉴权前）→ Bearer → Gzip → 路由。
// 该顺序由 middleware::apply 组装，并有测试锁定。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::Router;

fn write_error(status: StatusCode, msg: &str) -> Response {
    let body = serde_json::to_string(&serde_json::json!({ "error": msg }))
        .unwrap_or_else(|_| "{\"error\":\"\"}".to_string());
    let mut b = body;
    b.push('\n');
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        b,
    )
        .into_response()
}

/// Bearer 校验：Authorization: Bearer <tok> 必须命中令牌集之一，否则
/// 401 + {"error":"unauthorized"} + WWW-Authenticate: Bearer。
pub fn bearer_mw(
    tokens: Arc<HashSet<String>>,
) -> impl Fn(
    Request,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'static>>
+ Clone
+ Send
+ 'static {
    move |req: Request, next: Next| {
        let tokens = Arc::clone(&tokens);
        Box::pin(async move {
            let auth = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            // Go：prefix（Bearer + 空格）区分大小写，token 精确匹配。
            let ok = auth.starts_with("Bearer ")
                && tokens.contains(auth.strip_prefix("Bearer ").unwrap_or(""));
            if !ok {
                let mut resp = write_error(StatusCode::UNAUTHORIZED, "unauthorized");
                let _ = resp.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    header::HeaderValue::from_static("Bearer"),
                );
                return resp;
            }
            next.run(req).await
        })
    }
}

/// CORS 白名单：Origin 命中白名单（或 "*"）时回写
/// Access-Control-Allow-Origin；OPTIONS 预检在鉴权之前应答 204（浏览器
/// 预检不带凭据）。
pub fn cors_mw(
    origins: Arc<HashSet<String>>,
) -> impl Fn(
    Request,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'static>>
+ Clone
+ Send
+ 'static {
    move |req: Request, next: Next| {
        let origins = Arc::clone(&origins);
        Box::pin(async move {
            let mut acao: Option<(String, bool)> = None;
            if let Some(origin) = req
                .headers()
                .get(header::ORIGIN)
                .and_then(|v| v.to_str().ok())
            {
                if origins.contains("*") {
                    acao = Some(("*".to_string(), false));
                } else if origins.contains(origin) {
                    acao = Some((origin.to_string(), true));
                }
            }
            let is_preflight = req.method() == Method::OPTIONS && acao.is_some();
            if is_preflight {
                // 预检在鉴权之前应答：浏览器的 OPTIONS 不带 Authorization。
                let mut resp = Response::new(axum::body::Body::empty());
                *resp.status_mut() = StatusCode::NO_CONTENT;
                let h = resp.headers_mut();
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    header::HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
                );
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    header::HeaderValue::from_static("Content-Type, Authorization"),
                );
                h.insert(
                    header::ACCESS_CONTROL_MAX_AGE,
                    header::HeaderValue::from_static("86400"),
                );
                return resp;
            }
            let mut resp = next.run(req).await;
            if let Some((origin, vary)) = acao {
                let h = resp.headers_mut();
                let _ = h.insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    header::HeaderValue::from_str(&origin)
                        .unwrap_or(header::HeaderValue::from_static("*")),
                );
                if vary {
                    let existing = h
                        .get(header::VARY)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let joined = if existing.is_empty() {
                        "Origin".to_string()
                    } else {
                        format!("{existing}, Origin")
                    };
                    let _ = h.insert(
                        header::VARY,
                        header::HeaderValue::from_str(&joined)
                            .unwrap_or(header::HeaderValue::from_static("Origin")),
                    );
                }
            }
            resp
        })
    }
}

/// 组装完整中间件链（由外到内）：CORS → Bearer → Gzip → 路由。
/// tower 的 layer 语义是「先 add 的靠内」，所以按 内→外 顺序加。
pub fn apply(
    router: Router,
    bearer_tokens: Vec<String>,
    cors_origins: Vec<String>,
    timeout: Duration,
) -> Router {
    let bearer: Arc<HashSet<String>> = Arc::new(bearer_tokens.into_iter().collect());
    let cors: Arc<HashSet<String>> = Arc::new(cors_origins.into_iter().collect());
    let mut r = router;
    // 最内：Go 对任意大小响应都压缩（tower-http 默认谓词 SizeAbove(32)，
    // 对齐 Go 用 SizeAbove(0)）。
    r = r.layer(
        tower_http::compression::CompressionLayer::new()
            .compress_when(tower_http::compression::predicate::SizeAbove::new(0)),
    );
    if !bearer.is_empty() {
        r = r.layer(axum::middleware::from_fn(bearer_mw(Arc::clone(&bearer))));
    }
    if !cors.is_empty() {
        r = r.layer(axum::middleware::from_fn(cors_mw(Arc::clone(&cors))));
    }
    if timeout > Duration::ZERO {
        r = r.layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ));
    }
    r
}
