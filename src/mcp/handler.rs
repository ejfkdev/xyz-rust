// 服务器实现：rmcp 3.1.x 的 ServerHandler trait。每条注册命令一个工具；
// 版本「钉定」经 supported_protocol_versions 交给 SDK 的协商管线（它同时
// 约束 discover、initialize 与每次请求的版本校验——比 Go 版 handler 内
// 逐请求检查更彻底，语义等价）。

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};

use crate::ctx::Ctx;
use crate::errors;
use crate::mcp::{Options, protocol_version};
use crate::registry::Registry;
use crate::spec::entry::Entry;

#[derive(Clone)]
pub struct XyzServer {
    impl_info: Implementation,
    instructions: Option<String>,
    tools: Vec<Tool>,
    by_name: Arc<BTreeMap<String, Arc<Entry>>>,
    pinned: Option<Vec<ProtocolVersion>>,
    allowed: Option<std::collections::BTreeSet<String>>,
    pub(crate) ctx: Arc<Ctx>,
    defaults: std::collections::HashMap<String, String>,
}

/// 构建服务器实现。未登记的协议版本在注册期报错（与「注册期即报错」
/// 一致）。
pub fn build(reg: &Registry, opts: &Options, ctx: Arc<Ctx>) -> errors::Result<XyzServer> {
    for v in &opts.versions {
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
    let pinned: Option<Vec<ProtocolVersion>> = if opts.versions.is_empty() {
        None
    } else {
        Some(
            opts.versions
                .iter()
                .filter_map(|v| protocol_version(v))
                .collect(),
        )
    };
    let allowed: Option<std::collections::BTreeSet<String>> = if opts.versions.is_empty() {
        None
    } else {
        Some(opts.versions.iter().cloned().collect())
    };
    let (name, version) = impl_name(opts);
    let impl_info = Implementation::new(name, version);

    let mut tools = Vec::new();
    let mut by_name = BTreeMap::new();
    for e in reg.all() {
        if e.mcp.skip || e.cli.daemon {
            continue; // 通道层面整体移除；daemon 只属于 CLI
        }
        let input_schema = Arc::new(
            crate::spec::schema::schema_to_value(&e.input_schema)
                .as_object()
                .cloned()
                .unwrap_or_default(),
        );
        let output_schema = e.output_schema.as_ref().map(|s| {
            Arc::new(
                crate::spec::schema::schema_to_value(s)
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            )
        });
        let mut tool = Tool::new_with_raw(
            e.name.clone(),
            Some(std::borrow::Cow::Owned(tool_description(&e))),
            input_schema,
        );
        if let Some(out) = output_schema {
            tool = tool.with_raw_output_schema(out);
        }
        if let Some(ann) = parse_annotations(&e) {
            tool = tool.with_annotations(ann);
        }
        tools.push(tool);
        by_name.insert(e.name.clone(), e);
    }

    Ok(XyzServer {
        impl_info,
        instructions: if opts.instructions.is_empty() {
            None
        } else {
            Some(opts.instructions.clone())
        },
        tools,
        by_name: Arc::new(by_name),
        pinned,
        allowed,
        ctx,
        defaults: opts.defaults.clone(),
    })
}

impl ServerHandler for XyzServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::default())
            .with_server_info(self.impl_info.clone())
            .with_protocol_version(ProtocolVersion::LATEST);
        if let Some(ins) = &self.instructions {
            info = info.with_instructions(ins.clone());
        }
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        match &self.pinned {
            Some(p) => Cow::Owned(p.clone()),
            None => Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools
            .iter()
            .find(|t| t.name == name && self.by_name.contains_key(name))
            .cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // 版本门（对齐 Go handler 内的 allowed 检查；SDK 侧已先校验，
        // 这里兜自定义子集）。
        let pv = context.protocol_version().map(|v| v.as_str().to_string());
        if let (Some(allowed), Some(pv)) = (&self.allowed, pv)
            && !allowed.contains(&pv)
        {
            let e = self
                .by_name
                .get(request.name.as_ref())
                .map(|e| e.name.clone())
                .unwrap_or_else(|| request.name.to_string());
            return Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                ContentBlock::text(format!(
                    "tool {e:?}: protocol version {pv:?} is not enabled on this server"
                )),
            ])));
        }
        let Some(entry) = self.by_name.get(request.name.as_ref()) else {
            return Err(ErrorData::method_not_found::<
                rmcp::model::CallToolRequestMethod,
            >());
        };
        let mut arguments: crate::spec::JsonMap = request.arguments.unwrap_or_default();
        // 接口默认值只补「客户端未提供」的键；显式入参优先（与 CLI/HTTP
        // 一致），不覆盖调用方传来的值。
        for (k, v) in entry.mcp_defaults() {
            if !arguments.contains_key(&k) {
                arguments.insert(k, v);
            }
        }
        // 通道级默认参数（--default k=v）：只补缺席键。
        for (k, v) in &self.defaults {
            if !arguments.contains_key(k) {
                arguments.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }
        match (entry.invoke)(&self.ctx, &arguments) {
            Ok(out) => Ok(success_response(out)),
            Err(e) => {
                let msg = match e.cause() {
                    Some(c) => c.to_string(),
                    None => e.to_string(),
                };
                Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                    ContentBlock::text(msg),
                ])))
            }
        }
    }
}

/// 成功结果 → CallToolResponse：保留块信封原样透传 Content 块
/// （spec §12.7）；其余走 §12.5 的双内容（文本 + structuredContent）。
pub(super) fn success_response(out: serde_json::Value) -> CallToolResponse {
    if let Some(blocks) = crate::blocks::extract(&out) {
        let mut result = CallToolResult::error(to_rmcp_blocks(&blocks));
        result.is_error = None;
        result.structured_content = Some(out);
        return CallToolResponse::Complete(result);
    }
    let text = render_text(&out);
    let mut result = CallToolResult::error(vec![ContentBlock::text(text)]);
    result.is_error = None;
    result.structured_content = Some(out);
    CallToolResponse::Complete(result)
}

fn render_text(v: &serde_json::Value) -> String {
    let mut buf = Vec::new();
    if crate::cli::render::render_value(&mut buf, v).is_err() {
        return v.to_string();
    }
    String::from_utf8_lossy(&buf).trim_end().to_string()
}

/// 块信封 → rmcp Content：协议级内容块保真透传（spec §12.7）。
fn to_rmcp_blocks(blocks: &[crate::blocks::Block]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|b| match b {
            crate::blocks::Block::Text { text } => ContentBlock::text(text.clone()),
            crate::blocks::Block::Image { mime_type, data } => {
                ContentBlock::image(data.clone(), mime_type.clone())
            }
            crate::blocks::Block::Audio { mime_type, data } => {
                ContentBlock::audio(data.clone(), mime_type.clone())
            }
        })
        .collect()
}

fn tool_description(e: &Entry) -> String {
    if !e.summary.is_empty() && !e.description.is_empty() {
        format!("{}\n\n{}", e.summary, e.description)
    } else if !e.description.is_empty() {
        e.description.clone()
    } else {
        e.summary.clone()
    }
}

fn parse_annotations(e: &Entry) -> Option<ToolAnnotations> {
    if e.mcp.annotations.is_empty() {
        return None;
    }
    let mut ann = ToolAnnotations::default();
    for a in &e.mcp.annotations {
        let (key, val) = match a.split_once(':') {
            Some((k, v)) => (k, v.trim()),
            None => (a.as_str(), ""),
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "read" => ann.read_only_hint = Some(true),
            "write" => ann.read_only_hint = Some(false),
            "destructive" => ann.destructive_hint = Some(true),
            "idempotent" => ann.idempotent_hint = Some(true),
            "openworld" => ann.open_world_hint = Some(true),
            "title" => ann.title = Some(val.to_string()),
            _ => {}
        }
    }
    Some(ann)
}

fn impl_name(opts: &Options) -> (String, String) {
    let name = if opts.name.is_empty() {
        crate::cli::app::bin_name()
    } else {
        opts.name.clone()
    };
    let version = if opts.version.is_empty() {
        "0.0.0".to_string() // Go impl 同款默认
    } else {
        opts.version.clone()
    };
    (name, version)
}
