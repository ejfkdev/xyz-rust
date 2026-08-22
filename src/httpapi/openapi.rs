// /openapi.json：OpenAPI 3 文档，与 MCP 前端同源（同一 InputSchema）。
// 对齐 Go openapi.go：info 元数据、按 "路径 方法" 排序的 paths、path/query
// 参数表、POST/PUT/PATCH 的 requestBody、响应表（200 带 outputSchema）。

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

use crate::errors;
use crate::registry::Registry;
use crate::spec::field::{FieldKind, FieldMeta};
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

pub fn openapi_handler(reg: &Registry) -> OpenApiDoc {
    OpenApiDoc(Arc::new(build_doc(reg)))
}

fn build_doc(reg: &Registry) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    let mut order: Vec<String> = Vec::new();
    for e in reg.all() {
        if e.http.skip || e.http.method.is_empty() || e.http.path.is_empty() {
            continue;
        }
        order.push(format!("{} {}", e.http.path, e.http.method));
    }
    order.sort();
    for key in order {
        let (path, method) = match key.rsplit_once(' ') {
            Some((p, m)) => (p.to_string(), m.to_string()),
            None => continue,
        };
        let Some(e) = reg
            .all()
            .into_iter()
            .find(|e| e.http.path == path && e.http.method == method)
        else {
            continue;
        };
        let op = build_operation(&e);
        let path_obj = paths
            .entry(path)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(obj) = path_obj.as_object_mut() {
            obj.insert(method.to_lowercase(), op);
        }
    }
    let mut doc = Map::new();
    doc.insert("openapi".to_string(), Value::String("3.0.3".to_string()));
    let mut info = Map::new();
    info.insert(
        "title".to_string(),
        Value::String("example service".to_string()),
    );
    info.insert("version".to_string(), Value::String("1".to_string()));
    doc.insert("info".to_string(), Value::Object(info));
    doc.insert("paths".to_string(), Value::Object(paths));
    Value::Object(doc)
}

fn build_operation(e: &crate::spec::Entry) -> Value {
    let mut op = Map::new();
    if !e.summary.is_empty() {
        op.insert("summary".to_string(), Value::String(e.summary.clone()));
    }
    // 参数表：http:"path" 与 http:"query" 字段（Go openapi.go 同口径）。
    let mut params: Vec<Value> = Vec::new();
    for f in &e.root.children {
        if f.skip {
            continue;
        }
        if f.http.location != "path" && f.http.location != "query" {
            continue;
        }
        let mut p = Map::new();
        p.insert(
            "name".to_string(),
            Value::String(crate::httpapi::http_name(f).to_string()),
        );
        p.insert("in".to_string(), Value::String(f.http.location.clone()));
        p.insert("required".to_string(), Value::Bool(f.required));
        let mut schema = Map::new();
        schema.insert(
            "type".to_string(),
            Value::String(schema_type(f).to_string()),
        );
        p.insert("schema".to_string(), Value::Object(schema));
        params.push(Value::Object(p));
    }
    if !params.is_empty() {
        op.insert("parameters".to_string(), Value::Array(params));
    }
    // 请求体：POST/PUT/PATCH 以 inputSchema 为 schema。
    if matches!(e.http.method.as_str(), "POST" | "PUT" | "PATCH") {
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

/// Go openapi.go schemaType 对应物：基础 JSON 类型名。
fn schema_type(f: &FieldMeta) -> &'static str {
    match f.kind {
        FieldKind::Bool => "boolean",
        FieldKind::I8
        | FieldKind::I16
        | FieldKind::I32
        | FieldKind::I64
        | FieldKind::U8
        | FieldKind::U16
        | FieldKind::U32
        | FieldKind::U64 => "integer",
        FieldKind::F32 | FieldKind::F64 => "number",
        FieldKind::Slice => "array",
        FieldKind::Struct => "object",
        _ => "string", // String/Duration/Time/Bytes/Ptr
    }
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
