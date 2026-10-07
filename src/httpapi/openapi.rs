// /openapi.json：OpenAPI 3 文档，与 MCP 前端同源（同一 InputSchema）。
// 对齐 Go openapi.go：info 元数据、按 "路径 方法" 排序的 paths、path/query
// 参数表、POST/PUT/PATCH 的 requestBody、响应表（200 带 outputSchema）。

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

use crate::errors;
use crate::registry::Registry;
use std::sync::Arc;

/// 与 EntryHandler 同路线的具名 handler：构建期快照文档。
#[derive(Clone)]
pub struct OpenApiDoc(Arc<Value>);

impl axum::handler::Handler<(), ()> for OpenApiDoc {
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;
    fn call(self, _req: Request, _state: ()) -> Self::Future {
        Box::pin(async move { respond_doc(&self.0) })
    }
}

pub fn openapi_handler(reg: &Registry, app_name: &str, app_version: &str) -> OpenApiDoc {
    OpenApiDoc(Arc::new(build_doc(reg, app_name, app_version)))
}

fn build_doc(reg: &Registry, app_name: &str, app_version: &str) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    // 每注册方法一个 operation（spec §11.3）：默认 GET+POST 的命令产出
    // get 与 post 两条。
    let mut order: Vec<(String, String, String)> = Vec::new();
    for e in reg.all() {
        if e.http.skip || e.cli.daemon || e.http.path.is_empty() {
            continue;
        }
        for method in crate::httpapi::http_methods(&e) {
            order.push((e.http.path.clone(), method, e.name.clone()));
        }
    }
    order.sort();
    for (path, method, name) in order {
        let Some(e) = reg.all().into_iter().find(|e| e.name == name) else {
            continue;
        };
        let op = build_operation(&e, &method);
        let path_obj = paths
            .entry(path)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(obj) = path_obj.as_object_mut() {
            obj.insert(method.to_lowercase(), op);
        }
    }
    let mut doc = Map::new();
    doc.insert("openapi".to_string(), Value::String("3.0.3".to_string()));
    // info 身份 = 应用身份（§11.6 同值）；解析不到时用参考回退。
    let mut info = Map::new();
    let title = if app_name.is_empty() {
        "example service".to_string()
    } else {
        app_name.to_string()
    };
    let version = if app_version.is_empty() {
        "1".to_string()
    } else {
        app_version.to_string()
    };
    info.insert("title".to_string(), Value::String(title));
    info.insert("version".to_string(), Value::String(version));
    doc.insert("info".to_string(), Value::Object(info));
    doc.insert("paths".to_string(), Value::Object(paths));
    Value::Object(doc)
}

fn build_operation(e: &crate::spec::Entry, method: &str) -> Value {
    let mut op = Map::new();
    if !e.summary.is_empty() {
        op.insert("summary".to_string(), Value::String(e.summary.clone()));
    }
    if !e.description.is_empty() {
        op.insert(
            "description".to_string(),
            Value::String(e.description.clone()),
        );
    }
    // 参数表（spec §11.3）：每个 path/query/header 字段一条，带线上名、
    // 位置、required（path 参数恒 true）、描述与富 schema（type + enum/
    // default/format——与 MCP inputSchema 同源的逐字段 schema）。
    // 未标注 location 的字段运行期按 query 绑定（httpapi/mod.rs 的
    // ""|"query" 臂），文档同口径收录，否则 openapi.json 漏掉全部默认
    // 字段（框架使用者几乎都不逐字段标注）。
    let mut params: Vec<Value> = Vec::new();
    for f in &e.root.children {
        if f.skip {
            continue;
        }
        let location = if f.http.location.is_empty() {
            "query"
        } else {
            f.http.location.as_str()
        };
        if location != "path" && location != "query" && location != "header" {
            continue;
        }
        let mut p = Map::new();
        p.insert(
            "name".to_string(),
            Value::String(crate::httpapi::http_name(f).to_string()),
        );
        p.insert("in".to_string(), Value::String(location.to_string()));
        let required = if location == "path" { true } else { f.required };
        p.insert("required".to_string(), Value::Bool(required));
        if !f.description.is_empty() {
            p.insert(
                "description".to_string(),
                Value::String(f.description.clone()),
            );
        }
        p.insert(
            "schema".to_string(),
            crate::spec::schema::schema_to_value(&crate::spec::schema::field_schema(f)),
        );
        params.push(Value::Object(p));
    }
    if !params.is_empty() {
        op.insert("parameters".to_string(), Value::Array(params));
    }
    // 请求体：POST/PUT/PATCH 以 inputSchema 为 schema。
    if matches!(method, "POST" | "PUT" | "PATCH") {
        let body = serde_json::json!({
            "content": {
                "application/json": {
                    "schema": crate::spec::schema::schema_to_value(&e.input_schema),
                }
            }
        });
        op.insert("requestBody".to_string(), body);
    }
    // 响应表。
    let mut ok_resp = Map::new();
    ok_resp.insert("description".to_string(), Value::String("ok".to_string()));
    if let Some(out) = &e.output_schema {
        ok_resp.insert(
            "content".to_string(),
            serde_json::json!({
                "application/json": {
                    "schema": crate::spec::schema::schema_to_value(out),
                }
            }),
        );
    }
    let mut responses = Map::new();
    responses.insert("200".to_string(), Value::Object(ok_resp));
    responses.insert(
        "400".to_string(),
        serde_json::json!({ "description": errors::Kind::InvalidInput.as_str() }),
    );
    responses.insert(
        "404".to_string(),
        serde_json::json!({ "description": errors::Kind::NotFound.as_str() }),
    );
    responses.insert(
        "500".to_string(),
        serde_json::json!({ "description": errors::Kind::Internal.as_str() }),
    );
    op.insert("responses".to_string(), Value::Object(responses));
    Value::Object(op)
}

fn respond_doc(doc: &Arc<Value>) -> Response {
    let s = serde_json::to_string_pretty(&**doc).unwrap_or_else(|_| "{}".to_string());
    let mut b = s;
    b.push('\n');
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        b,
    )
        .into_response()
}
