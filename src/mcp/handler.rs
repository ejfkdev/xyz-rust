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
    ListToolsResult, MetaObject, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerConfig, Tool, ToolAnnotations,
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
    /// initialize 结果声明的协议版本：钉定集内最新，否则
    /// LATEST_WITH_INITIALIZE（见 build 内说明）。
    info_version: ProtocolVersion,
    pub(crate) ctx: Arc<Ctx>,
    defaults: std::collections::HashMap<String, String>,
    /// 结果 _meta.xyz 的上下文（§12.8）：None = 关闭（no_server_meta）。
    meta_ctx: Option<MetaCtx>,
}

/// _meta.xyz 的静态身份分量（每个结果复用）。
#[derive(Clone)]
struct MetaCtx {
    app_name: String,
    app_version: String,
    headers: Vec<(String, String)>,
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
    // initialize 结果只能命名「仍带 initialize 握手」的版本，且不应超出
    // --versions 钉定集——rmcp 的协商兜底直接采用该值、不核对成员资格，
    // 钉定集外的值会「漏」给客户端（钉 2024-11-05 仍答 2025-11-25 的根因）。
    let info_version = pinned
        .as_ref()
        .and_then(|p| p.iter().max_by(|a, b| a.as_str().cmp(b.as_str())).cloned())
        .unwrap_or(ProtocolVersion::LATEST_WITH_INITIALIZE);
    let (name, version) = impl_name(opts);
    let (meta_name, meta_version) = (name.clone(), version.clone());
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
        // outputSchema 顶层必须是 object（MCP：给了 schema 就必须提供合规的
        // structuredContent，而数组/标量 schema 不可能存在合规结果）——
        // 非 object 的结果类型不发射 schema，结果只走 textContent（ddc#9）。
        let output_schema = e
            .output_schema
            .as_ref()
            .filter(|s| s.r#type.as_deref() == Some("object"))
            .map(|s| {
                Arc::new(
                    crate::spec::schema::schema_to_value(s)
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                )
            });
        // spec §12.4：描述可经 MCPHints.description 覆写，否则 summary+
        // description 合并；title 经 hints 或 title:… 注解写入
        // annotations.title；hints.meta 并入工具的保留 _meta。
        let desc = if e.mcp.description.is_empty() {
            tool_description(&e)
        } else {
            e.mcp.description.clone()
        };
        let mut tool = Tool::new_with_raw(
            tool_name(&e),
            Some(std::borrow::Cow::Owned(desc)),
            input_schema,
        );
        if let Some(out) = output_schema {
            tool = tool.with_raw_output_schema(out);
        }
        match parse_annotations(&e) {
            Some(mut ann) => {
                if !e.mcp.title.is_empty() {
                    ann.title = Some(e.mcp.title.clone());
                }
                tool = tool.with_annotations(ann);
            }
            None => {
                if !e.mcp.title.is_empty() {
                    let mut ann = ToolAnnotations::default();
                    ann.title = Some(e.mcp.title.clone());
                    tool = tool.with_annotations(ann);
                }
            }
        }
        if !e.mcp.meta.is_empty() {
            tool.meta = Some(MetaObject(e.mcp.meta.clone()));
        }
        tools.push(tool);
        by_name.insert(tool_name(&e), e);
    }

    let meta_ctx = if opts.no_server_meta {
        None
    } else {
        // serverInfo 与 _meta 用同一身份（§12.6/§12.8；impl_name 已解析
        // basename / 默认版本）。
        Some(MetaCtx {
            app_name: meta_name,
            app_version: meta_version,
            headers: opts.response_headers.clone(),
        })
    };
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
        info_version,
        ctx,
        defaults: opts.defaults.clone(),
        meta_ctx,
    })
}

impl ServerHandler for XyzServer {
    fn get_info(&self) -> ServerConfig {
        // MCP 要求支持 tools 的服务器 MUST 声明 tools 能力，否则按能力协商
        // 的客户端不发 tools/list、工具面显示为 0；工具集构建期固定且不发
        // list_changed 通知，故不启用 enable_tool_list_changed。
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(self.impl_info.clone())
            .with_protocol_version(self.info_version.clone());
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
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(shape_tools(
            self.tools.clone(),
            context.protocol_version().as_ref(),
        )))
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
        let started = std::time::Instant::now();
        match (entry.invoke)(&self.ctx, &arguments) {
            Ok(out) => {
                let meta = self.meta_for(&entry.name, started.elapsed(), None);
                let structured = supports_structured(context.protocol_version().as_ref());
                Ok(success_response(out, meta, structured))
            }
            Err(e) => {
                let msg = match e.cause() {
                    Some(c) => c.to_string(),
                    None => e.to_string(),
                };
                // §8.6 + §12.8：失败结果带共享错误体投影的 _meta.xyz.error。
                let body = errors::error_body(&e);
                let meta = self.meta_for(&entry.name, started.elapsed(), Some(&body));
                let mut result = CallToolResult::error(vec![ContentBlock::text(msg)]);
                result.meta = meta;
                Ok(CallToolResponse::Complete(result))
            }
        }
    }
}

impl XyzServer {
    /// 单次调用结果的 _meta.xyz（§12.8）；关闭时 None。
    fn meta_for(
        &self,
        command: &str,
        duration: std::time::Duration,
        err: Option<&errors::Body>,
    ) -> Option<MetaObject> {
        let ctx = self.meta_ctx.as_ref()?;
        Some(result_meta(
            &ctx.app_name,
            &ctx.app_version,
            &ctx.headers,
            command,
            duration.as_millis(),
            err,
        ))
    }
}

/// 构造 _meta.xyz 服务器上下文（spec §12.8，形状对齐 HTTP §11.6 头）：
/// 应用身份 + xyz 库版本 + 命令 + 耗时 + 可选自定义头与错误上下文
/// （kind/code/detail，让 MCP 客户端无需解析文本即可分支）。
pub(super) fn result_meta(
    app_name: &str,
    app_version: &str,
    headers: &[(String, String)],
    command: &str,
    duration_ms: u128,
    err: Option<&errors::Body>,
) -> MetaObject {
    let mut xyz = serde_json::Map::new();
    xyz.insert("app_name".into(), serde_json::json!(app_name));
    xyz.insert("app_version".into(), serde_json::json!(app_version));
    xyz.insert(
        "sdk_version".into(),
        serde_json::json!(crate::version::SDK_VERSION),
    );
    xyz.insert("command".into(), serde_json::json!(command));
    xyz.insert("duration_ms".into(), serde_json::json!(duration_ms));
    if !headers.is_empty() {
        let mut hs = serde_json::Map::new();
        for (k, v) in headers {
            hs.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        xyz.insert("headers".into(), serde_json::Value::Object(hs));
    }
    if let Some(b) = err {
        let mut em = serde_json::Map::new();
        em.insert("kind".into(), serde_json::json!(b.kind.as_str()));
        if let Some(code) = &b.code {
            em.insert("code".into(), serde_json::json!(code));
        }
        if let Some(detail) = &b.detail {
            em.insert("detail".into(), serde_json::Value::Object(detail.clone()));
        }
        xyz.insert("error".into(), serde_json::Value::Object(em));
    }
    let mut root = serde_json::Map::new();
    root.insert("xyz".into(), serde_json::Value::Object(xyz));
    MetaObject(root)
}

/// 成功结果 → CallToolResponse：保留块信封原样透传 Content 块
/// （spec §12.7）；其余走 §12.5 的双内容（文本 + structuredContent）。
pub(super) fn success_response(
    out: serde_json::Value,
    meta: Option<MetaObject>,
    structured: bool,
) -> CallToolResponse {
    // structuredContent 必须是 JSON object（MCP 规范），且该字段与
    // outputSchema 自 2025-06-18 修订才入规——不满足时只走 textContent
    // （ddc#9：数组/标量结果此前发裸值，严格客户端直接拒绝）。
    let structured = structured && out.is_object();
    if let Some(blocks) = crate::blocks::extract(&out) {
        let mut result = CallToolResult::error(to_rmcp_blocks(&blocks));
        result.is_error = None;
        result.structured_content = structured.then_some(out);
        result.meta = meta;
        return CallToolResponse::Complete(result);
    }
    let text = render_text(&out);
    let mut result = CallToolResult::error(vec![ContentBlock::text(text)]);
    result.is_error = None;
    result.structured_content = structured.then_some(out);
    result.meta = meta;
    CallToolResponse::Complete(result)
}

/// outputSchema / structuredContent 自 2025-06-18 修订入规；更早的协商版本
/// 一律不发射（`--versions` 钉旧版因此是有效的逃生门）。
pub(super) fn supports_structured(client: Option<&ProtocolVersion>) -> bool {
    client.is_none_or(|v| *v >= ProtocolVersion::V_2025_06_18)
}

/// tools/list 整形：客户端协商版本 < 2025-06-18 时逐请求剥离
/// outputSchema（工具表构建期只带合规 schema）。
pub(super) fn shape_tools(mut tools: Vec<Tool>, client: Option<&ProtocolVersion>) -> Vec<Tool> {
    if !supports_structured(client) {
        for t in &mut tools {
            t.output_schema = None;
        }
    }
    tools
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

/// MCP 工具名：MCPHints.name 覆写优先（spec §12.4a），否则点分注册名。
fn tool_name(e: &Entry) -> String {
    if e.mcp.name.is_empty() {
        e.name.clone()
    } else {
        e.mcp.name.clone()
    }
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
