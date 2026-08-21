// httpapi 测试（Go httpapi_test.go 对应物）：绑定合并、错误映射、healthz、
// openapi、Bearer/CORS 中间件。用 tower::ServiceExt::oneshot 直接调
// Router，不真起端口。

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use tower::ServiceExt;

use crate::Ctx;
use crate::errors;
use crate::httpapi;
use crate::registry::Registry;
use crate::spec::command::{Command, HTTPHints};
use xyz_rust::XyzArgs;

#[derive(XyzArgs)]
struct AddHTTPArgs {
    #[xyz(desc = "用户名", required, validate = "min=2", http = "path")]
    name: String,
    #[xyz(desc = "年龄", default = "18", http = "query")]
    age: i32,
    #[xyz(desc = "令牌", skip, http = "header", http_name = "X-Token")]
    token: String,
}

fn add_user(_: &Ctx, in_: &AddHTTPArgs) -> errors::Result<String> {
    Ok(format!("{}:{}:{}", in_.name, in_.age, in_.token))
}

async fn resp_body(resp: Response) -> (u16, String) {
    let status = resp.status().as_u16();
    let body = resp.into_body();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn call(router: axum::routing::Router, req: Request<Body>) -> (u16, String) {
    let resp = router.oneshot(req).await.unwrap();
    resp_body(resp).await
}

fn add_reg() -> Registry {
    let reg = Registry::new();
    Command::new("user.add", add_user)
        .summary("加用户")
        .http(HTTPHints {
            method: "POST".into(),
            path: "/users/{name}".into(),
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    reg
}

#[tokio::test(flavor = "current_thread")]
async fn binding_merges_body_path_query_header() {
    let reg = add_reg();
    let router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    let body = serde_json::json!({"age": 9}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/users/alice?age=7")
        .header("X-Token", "tk")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, out) = call(router, req).await;
    assert_eq!(status, 200);
    // query 显式提供优先于 body 基底（body 先铺底，query 后覆盖——Go 同序）
    assert_eq!(out.trim(), "\"alice:7:tk\"");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_declared_json_body_is_400() {
    let reg = add_reg();
    let router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    // 显式声明 application/json 且解析失败 → 严格 400
    let req = Request::builder()
        .method("POST")
        .uri("/users/alice")
        .header("content-type", "application/json")
        .body(Body::from("{nope".to_string()))
        .unwrap();
    let (status, out) = call(router, req).await;
    assert_eq!(status, 400);
    assert!(out.contains("invalid JSON body"), "{out}");
}

#[tokio::test(flavor = "current_thread")]
async fn undeclared_body_is_not_rejected_as_json() {
    // 非 JSON 声明的不可解析体（如表单体）不提前判死：交给 form 绑定/其余字段。
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct F {
        #[xyz(desc = "n", http = "form")]
        note: String,
    }
    fn fh(_: &Ctx, in_: &F) -> errors::Result<String> {
        Ok(format!("note={}", in_.note))
    }
    Command::new("f.submit", fh)
        .http(HTTPHints {
            method: "POST".into(),
            path: "/f".into(),
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    let router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    // 无 Content-Type 的 form 体可以解出，不落 400
    let req = Request::builder()
        .method("POST")
        .uri("/f")
        .body(Body::from("note=hello%20form".to_string()))
        .unwrap();
    let (status, out) = call(router.clone(), req).await;
    assert_eq!(status, 200);
    assert_eq!(out.trim(), "\"note=hello form\"");
    // form 值只取请求体，不混入 query（与上游 v0.2.2 一致）
    let req = Request::builder()
        .method("POST")
        .uri("/f?note=fromquery")
        .body(Body::from("note=frombody".to_string()))
        .unwrap();
    let (_, out) = call(router, req).await;
    assert_eq!(out.trim(), "\"note=frombody\"");
}

#[tokio::test(flavor = "current_thread")]
async fn validation_error_maps_to_400() {
    let reg = add_reg();
    let router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/users/a") // min=2 失败
        .body(Body::empty())
        .unwrap();
    let (status, out) = call(router, req).await;
    assert_eq!(status, 400);
    assert!(
        out.contains("invalid value for field \\\"name\\\": min"),
        "{out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn business_error_maps_to_404_with_error_body() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct N {
        #[xyz(desc = "n")]
        n: String,
    }
    fn nh(_: &Ctx, _: &N) -> errors::Result<String> {
        Err(errors::new(errors::Kind::NotFound, "no such user"))
    }
    Command::new("n.hit", nh)
        .http(HTTPHints {
            method: "GET".into(),
            path: "/n".into(),
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    let router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    let req = Request::builder()
        .method("GET")
        .uri("/n")
        .body(Body::empty())
        .unwrap();
    let (status, out) = call(router, req).await;
    assert_eq!(status, 404);
    assert_eq!(out.trim(), "{\"error\":\"no such user\"}");
}

#[tokio::test(flavor = "current_thread")]
async fn healthz_and_openapi() {
    let reg = add_reg();
    let router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    let req = Request::builder()
        .method("GET")
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let (status, out) = call(router.clone(), req).await;
    assert_eq!(status, 200);
    assert_eq!(out, "{\"status\":\"ok\"}\n");
    let req = Request::builder()
        .method("GET")
        .uri("/openapi.json")
        .body(Body::empty())
        .unwrap();
    let (status, out) = call(router, req).await;
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["openapi"], "3.0.3");
    let path = v["paths"]["/users/{name}"]["post"].clone();
    assert_eq!(path["summary"], "加用户");
    // 参数表：path 里的 name + requestBody
    let params = path["parameters"].as_array().unwrap();
    assert!(
        params
            .iter()
            .any(|p| p["name"] == "name" && p["in"] == "path"),
        "{params:?}"
    );
    assert!(path["requestBody"]["content"]["application/json"]["schema"]["type"] == "object");
}

#[test]
fn conflicting_routes_rejected() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct X {}
    fn xh(_: &Ctx, _: &X) -> errors::Result<String> {
        Ok(String::new())
    }
    Command::new("x.one", xh)
        .http(HTTPHints {
            method: "GET".into(),
            path: "/dup".into(),
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    Command::new("x.two", xh)
        .http(HTTPHints {
            method: "GET".into(),
            path: "/dup".into(),
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    let err = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap_err();
    assert!(err.to_string().contains("conflicts"), "{err}");
    // 同路径不同方法不冲突
    let reg2 = Registry::new();
    Command::new("x.one", xh)
        .http(HTTPHints {
            method: "GET".into(),
            path: "/d".into(),
            ..Default::default()
        })
        .register(&reg2)
        .unwrap();
    Command::new("x.two", xh)
        .http(HTTPHints {
            method: "POST".into(),
            path: "/d".into(),
            ..Default::default()
        })
        .register(&reg2)
        .unwrap();
    assert!(httpapi::router(&reg2, Arc::new(Ctx::new())).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn handler_for_binds_one_entry() {
    let reg = add_reg();
    let entry = reg.get("user.add").unwrap();
    let h = httpapi::handler_for(entry);
    let router: axum::routing::Router =
        axum::routing::Router::new().route("/users/{name}", axum::routing::post(h));
    let req = Request::builder()
        .method("POST")
        .uri("/users/some?age=3")
        .header("X-Token", "z")
        .body(Body::empty())
        .unwrap();
    let (status, out) = call(router, req).await;
    assert_eq!(status, 200);
    assert_eq!(out.trim(), "\"some:3:z\""); // 挂载点沿用原路由模板，path 参数才能被摘出（Go HandlerFor 同契约）
}

#[tokio::test(flavor = "current_thread")]
async fn bearer_and_cors_middleware() {
    let reg = add_reg();
    let mut router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    router = crate::httpapi::middleware::apply(
        router,
        vec!["s3cret".into()],
        vec!["http://app.local".into()],
        std::time::Duration::ZERO,
    );
    // 无凭据 401 + WWW-Authenticate
    let req = Request::builder()
        .method("GET")
        .uri("/nope")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    assert_eq!(resp.headers().get("www-authenticate").unwrap(), "Bearer");
    // 带凭据进入路由（404 因为 /nope 无路由——鉴权已通过）
    let req = Request::builder()
        .method("GET")
        .uri("/nope")
        .header("Authorization", "Bearer s3cret")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    // CORS 预检（无凭据）在鉴权之前应答 204——顺序锁死
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/nope")
        .header("Origin", "http://app.local")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 204);
}

#[tokio::test(flavor = "current_thread")]
async fn gzip_compresses_when_asked() {
    let reg = add_reg();
    let mut router = httpapi::router(&reg, Arc::new(Ctx::new())).unwrap();
    router = crate::httpapi::middleware::apply(router, vec![], vec![], std::time::Duration::ZERO);
    let req = Request::builder()
        .method("GET")
        .uri("/healthz")
        .header("Accept-Encoding", "gzip")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("content-encoding").unwrap(), "gzip");
}
