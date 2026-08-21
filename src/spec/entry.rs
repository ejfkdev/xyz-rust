// Entry 是一条命令的类型擦除视图：前端唯一可见的面。具体入参与结果
// 类型藏在 Invoke 闭包内部。

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::ctx::Ctx;
use crate::errors;
use crate::spec::JsonMap;
use crate::spec::command::{CliHints, HTTPHints, MCPHints};
use crate::spec::field::FieldMeta;
use crate::spec::schema::Schema;

pub struct Entry {
    pub name: String,
    pub summary: String,
    pub description: String,

    /// 入参 struct 的 JSON Schema；MCP 前端与 OpenAPI 文档直接消费。
    pub input_schema: Schema,

    /// handler 结果类型的 JSON Schema（MCP tool.outputSchema / OpenAPI
    /// 响应 schema）。结果类型无法 schematize 时为 None。
    pub output_schema: Option<Schema>,

    /// 入参 struct 树（tags、默认值、CLI/HTTP 绑定）——生成 flag、路由
    /// 或绑定器的前端读这里。
    pub root: FieldMeta,

    pub cli: CliHints,
    pub http: HTTPHints,
    pub mcp: MCPHints,

    /// 解码入参——任何前端归约出的 map 形状——补默认、校验并跑 handler。
    /// 解码与校验失败一律以 Kind::InvalidInput 分类返回。
    #[allow(clippy::type_complexity)]
    pub invoke: Box<dyn Fn(&Ctx, &JsonMap) -> errors::Result<Value> + Send + Sync>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("name", &self.name)
            .field("input_schema", &self.input_schema)
            .field("output_schema", &self.output_schema)
            .field("cli", &self.cli)
            .field("http", &self.http)
            .field("mcp", &self.mcp)
            .finish()
    }
}

impl Entry {
    /// sf: 供前端覆写——默认实现为空。
    pub(crate) fn _ty() {}

    /// CLI 专属默认值：每个可绑定字段一条，按 JSON 名键控。CLI 前端在
    /// Invoke 之前把它们注入入参 map；全局 tag 默认由 Invoke 自己补，
    /// 所以优先级是 CLI 覆盖 > 全局 tag > 零值。
    pub fn cli_defaults(&self) -> Map<String, Value> {
        transport_defaults(&self.root, |f| f.cli.default.clone())
    }

    /// HTTP 专属默认值。
    pub fn http_defaults(&self) -> Map<String, Value> {
        transport_defaults(&self.root, |f| f.http.default.clone())
    }

    /// MCP 专属默认值。MCP 的覆盖同时替换 inputSchema 里的 default。
    pub fn mcp_defaults(&self) -> Map<String, Value> {
        transport_defaults(&self.root, |f| f.mcp.default.clone())
    }
}

fn transport_defaults(
    node: &FieldMeta,
    get: impl Fn(&FieldMeta) -> Option<Value> + Copy,
) -> Map<String, Value> {
    let mut out = Map::new();
    for f in &node.children {
        if f.skip {
            continue;
        }
        if let Some(d) = get(f) {
            out.insert(f.json_name.clone(), d);
        }
    }
    out
}

/// 便捷别名：多条 Entry 的共享句柄。
pub type SharedEntry = Arc<Entry>;
