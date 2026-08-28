// Command 是单条命令定义的构建器 + 三套 Define-time 配置提示。
// define 打开一条定义，链上 transport 提示后调用 entry/register。
//
// handler 的错误类型 E 是开放泛型（任意 std::error::Error）：Invoke 在
// 构建时把它擦掉（errors::upgrade 沿 source 链保留原分类）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::ctx::Ctx;
use crate::errors;
use crate::registry;
use crate::spec::field::FieldMeta;
use crate::spec::schema::build_schema;
use crate::spec::{Entry, JsonMap, XyzArgs, XyzSchema};

/// CliHints 是 CLI 前端的命令级配置。字段级绑定可以放在入参 struct 的
/// cli 属性上，也可以放在 Fields（按字段 JSON 名或 Rust 名索引）。Fields
/// 条目合并覆盖属性绑定——零值 hint 字段保留属性值，因此可以只在属性里
/// 设短名、只在构建器里改默认值。
#[derive(Debug, Clone, Default)]
pub struct CliHints {
    /// 单行调用式，如 "add <name>"。
    pub usage: String,
    /// 等价的子命令拼写。
    pub aliases: Vec<String>,
    /// 从帮助列表里隐藏。
    pub hidden: bool,
    /// 使该命令成为其父节点的默认子命令：首段不是已注册命令段（且不是
    /// flag）时，整串参数不消费地转发给它（udf image.tar ⇔
    /// udf extract image.tar，当 extract 标记了 default）。
    pub default: bool,
    /// 从 CLI 通道整体移除该命令：不建子命令、别名不生效、不出现在
    /// completion。与 hidden 的区别：hidden 只藏帮助、仍可执行。
    pub skip: bool,
    /// 声明「长驻命令」：handler 阻塞到 ctx 取消再返回。语义：隐含
    /// HTTP/MCP 双排除（通道层面不消费）；执行时不渲染返回值（handler
    /// 的 error 照常分类）；ctx 取消即优雅关停、退出 0。
    pub daemon: bool,
    /// `-h` 帮助的自定义文本块：分别插在帮助最前（description 之前）与
    /// 最后（Global Flags 之后）。原样输出（多行、缩进自控；结尾换行归一）。
    /// 空 = 不插入。仅叶子命令生效（中间节点没有 CliHints）。
    pub before: String,
    pub after: String,
    pub fields: HashMap<String, CliFieldHint>,
}

/// Define 期的 CLI 字段级配置。
#[derive(Debug, Clone, Default)]
pub struct CliFieldHint {
    /// 单字符短名，如 'a'。
    pub shorthand: Option<String>,
    pub positional: bool,
    pub hidden: bool,
    /// 对 CLI 前端不可见（属性等价物：cli = "-"）。
    pub skip: bool,
    /// 未显式提供时回退到这个环境变量。
    pub env_var: Option<String>,
    /// CLI-only 默认值；对 CLI 前端覆盖全局 default 属性。
    pub default: Option<Value>,
}

/// HTTPHints 是 HTTP 前端的命令级配置。
#[derive(Debug, Clone, Default)]
pub struct HTTPHints {
    pub method: String,
    /// 路由模板，如 "/users/{name}"。
    pub path: String,
    /// 每请求超时覆盖；0 保持前端默认。
    pub timeout: Duration,
    /// 从 HTTP 通道整体移除该命令：不注册路由、不进 /openapi.json。
    pub skip: bool,
    pub fields: HashMap<String, HTTPFieldHint>,
}

/// Define 期的 HTTP 字段级配置。
#[derive(Debug, Clone, Default)]
pub struct HTTPFieldHint {
    /// "" | query | path | header | form | body。
    pub location: String,
    /// 线上名覆盖（通常是 header 名）。
    pub name: Option<String>,
    pub default: Option<Value>,
}

/// MCPHints 是 MCP 前端的命令级配置。
#[derive(Debug, Clone, Default)]
pub struct MCPHints {
    /// 形如 "read"、"write"、"destructive"、"title:创建用户"。
    pub annotations: Vec<String>,
    /// 从 MCP 通道整体移除该命令：不成为工具。
    pub skip: bool,
    pub fields: HashMap<String, MCPFieldHint>,
}

/// Define 期的 MCP 字段级配置。MCP 的覆盖同时替换 inputSchema 里的
/// default。
#[derive(Debug, Clone, Default)]
pub struct MCPFieldHint {
    pub default: Option<Value>,
}

/// Command 是一条命令的定义构造器。
pub struct Command<T, R, F, E>
where
    T: XyzArgs,
    R: XyzSchema + Serialize,
    F: Fn(&Ctx, &T) -> std::result::Result<R, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) description: String,
    pub(crate) handler: Arc<F>,
    pub(crate) cli: CliHints,
    pub(crate) http: HTTPHints,
    pub(crate) mcp: MCPHints,
    pub(crate) _marker: std::marker::PhantomData<(T, R, E)>,
}

impl<T, R, F, E> Command<T, R, F, E>
where
    T: XyzArgs,
    R: XyzSchema + Serialize,
    F: Fn(&Ctx, &T) -> std::result::Result<R, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    /// 打开一条命令定义。name 必须匹配 ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$
    /// （MCP tool 名兼容）；检查在 entry() 执行，所以 define 本身不会失败。
    pub fn new(name: &str, handler: F) -> Self {
        Command {
            name: name.to_string(),
            summary: String::new(),
            description: String::new(),
            handler: Arc::new(handler),
            cli: CliHints::default(),
            http: HTTPHints::default(),
            mcp: MCPHints::default(),
            _marker: std::marker::PhantomData,
        }
    }

    /// 单行描述（帮助文本与 MCP 工具列表使用）。
    pub fn summary(mut self, s: impl Into<String>) -> Self {
        self.summary = s.into();
        self
    }

    /// 更长的解释（完整帮助与工具文档展示）。
    pub fn description(mut self, s: impl Into<String>) -> Self {
        self.description = s.into();
        self
    }

    /// 附加命令级 CLI 配置。
    pub fn cli(mut self, h: CliHints) -> Self {
        self.cli = h;
        self
    }

    /// 附加命令级 HTTP 配置。
    pub fn http(mut self, h: HTTPHints) -> Self {
        self.http = h;
        self
    }

    /// 附加命令级 MCP 配置。
    pub fn mcp(mut self, h: MCPHints) -> Self {
        self.mcp = h;
        self
    }

    /// 注册名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 分析一次入参类型并构建前端可见的 Entry（不注册）。所有定义错误
    /// （坏名字、坏 tag、不支持的形状）都在这里浮出，而不是首次调用时。
    pub fn entry(&self) -> errors::Result<Entry> {
        check_entry_name(&self.name).map_err(|e| {
            errors::Error::new(e.kind(), format!("spec: command {:?}: {}", self.name, e))
        })?;
        // xyz_spec 的递归护栏用 panic 表达；注册期把它捕获成错误。
        let meta = match std::panic::catch_unwind(T::xyz_meta) {
            Ok(Ok(meta)) => meta.to_vec(),
            Ok(Err(e)) => {
                return Err(errors::Error::new(
                    e.kind(),
                    format!("spec: command {:?}: {}", self.name, e),
                ));
            }
            Err(panic) => {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "argument type analysis failed".to_string());
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!("spec: command {:?}: {msg}", self.name),
                ));
            }
        };
        // 每个 Entry 一份可变的元数据树（hints 合并写在这份上）。
        let mut fields: Vec<FieldMeta> = meta;
        apply_hints(&self.name, &mut fields, &self.cli, &self.http, &self.mcp)?;

        // Define-time 默认值的注册期校验（对齐 Go 的 normalizeHintDefault）。
        for (idx, f) in fields.iter().enumerate() {
            if let Some(d) = &f.cli.default {
                T::xyz_type_check(idx, &fields, d).map_err(|e| {
                    errors::Error::new(
                        e.kind(),
                        format!("spec: command {:?}: cli field {:?}: {e}", self.name, f.name),
                    )
                })?;
            }
            if let Some(d) = &f.http.default {
                T::xyz_type_check(idx, &fields, d).map_err(|e| {
                    errors::Error::new(
                        e.kind(),
                        format!(
                            "spec: command {:?}: http field {:?}: {e}",
                            self.name, f.name
                        ),
                    )
                })?;
            }
            if let Some(d) = &f.mcp.default {
                T::xyz_type_check(idx, &fields, d).map_err(|e| {
                    errors::Error::new(
                        e.kind(),
                        format!("spec: command {:?}: mcp field {:?}: {e}", self.name, f.name),
                    )
                })?;
            }
        }

        let input_schema = build_schema(&fields);
        let meta_fields = fields.clone();

        // 共享管线：map -> 强类型（默认/required/枚举）-> 校验 -> handler。
        let h = Arc::clone(&self.handler);
        #[allow(clippy::type_complexity)]
        let invoke: Box<dyn Fn(&Ctx, &JsonMap) -> errors::Result<Value> + Send + Sync> =
            Box::new(move |ctx: &Ctx, args: &JsonMap| {
                let decoded = T::xyz_decode(args, &meta_fields)?;
                T::xyz_validate(&decoded, &meta_fields)?;
                let out = match (h)(ctx, &decoded) {
                    Ok(out) => out,
                    Err(e) => {
                        // 沿用户错误链保留既有分类（Go：handler 的错误类型
                        // 直接参与 classify）；未分类才兜底 internal。
                        return Err(match errors::classify(&e) {
                            Some(kind) => errors::Error::wrap(kind, e),
                            None => errors::Error::upgrade(e),
                        });
                    }
                };
                serde_json::to_value(out).map_err(|e| {
                    errors::Error::new(errors::Kind::Internal, format!("result serialization: {e}"))
                })
            });

        let root = FieldMeta {
            name: String::new(),
            json_name: String::new(),
            kind: crate::spec::field::FieldKind::Struct,
            description: String::new(),
            required: false,
            secret: false,
            validate: String::new(),
            rules: Vec::new(),
            enum_values: Vec::new(),
            default: None,
            skip: false,
            cli: Default::default(),
            http: Default::default(),
            mcp: Default::default(),
            elem: None,
            children: fields,
            union: None,
        };

        Ok(Entry {
            name: self.name.clone(),
            summary: self.summary.clone(),
            description: self.description.clone(),
            input_schema,
            output_schema: R::xyz_schema(),
            root,
            cli: self.cli.clone(),
            http: self.http.clone(),
            mcp: self.mcp.clone(),
            invoke,
        })
    }

    /// 构建 Entry 并注册到 r。
    pub fn register(self, r: &registry::Registry) -> errors::Result<std::sync::Arc<Entry>> {
        let e = self.entry()?;
        r.add(Arc::new(e))
    }
}

/// entry 名的同构检查：MCP tool 名兼容（^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$），
/// 同时约束 CLI 子命令名与 HTTP 路由段。
pub fn check_entry_name(name: &str) -> errors::Result<()> {
    let mut chars = name.chars();
    let ok = match chars.next() {
        Some(c) => c.is_ascii_alphanumeric(),
        None => false,
    } && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !ok {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!("name must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,127}}$, got {name:?}"),
        ));
    }
    Ok(())
}

/// hints 合并（Go applyHints）：Fields 键为顶层字段的 JSON 名或 Rust 名；
/// 零值 hint 字段保留属性绑定。
pub fn apply_hints(
    cmd_name: &str,
    fields: &mut [FieldMeta],
    cli: &CliHints,
    http: &HTTPHints,
    mcp: &MCPHints,
) -> errors::Result<()> {
    let names = fields
        .iter()
        .map(|f| format!("{:?}", f.json_name))
        .collect::<Vec<_>>()
        .join(", ");
    for (key, hint) in &cli.fields {
        let Some(i) = lookup_top_field(fields, key) else {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!("cli field {key:?}: unknown field {key:?} (fields: {names})"),
            ));
        };
        apply_cli_hint(&mut fields[i], key, hint)?;
    }
    for (key, hint) in &http.fields {
        let Some(i) = lookup_top_field(fields, key) else {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!("http field {key:?}: unknown field {key:?} (fields: {names})"),
            ));
        };
        apply_http_hint(&mut fields[i], key, hint)?;
    }
    for (key, hint) in &mcp.fields {
        let Some(i) = lookup_top_field(fields, key) else {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!("mcp field {key:?}: unknown field {key:?} (fields: {names})"),
            ));
        };
        apply_mcp_hint(&mut fields[i], key, hint)?;
    }
    let _ = cmd_name;
    Ok(())
}

/// JSON 名 → Rust 名（精确）→ Rust 名（忽略大小写）；点号键拒绝
/// （嵌套配置暂不支持）。返回字段索引供调用方取可变引用。
pub fn lookup_top_field(fields: &[FieldMeta], key: &str) -> Option<usize> {
    if key.contains('.') {
        return None;
    }
    for (i, f) in fields.iter().enumerate() {
        if f.json_name == key || f.name == key {
            return Some(i);
        }
    }
    fields.iter().position(|f| f.name.eq_ignore_ascii_case(key))
}

pub fn apply_cli_hint(f: &mut FieldMeta, key: &str, h: &CliFieldHint) -> errors::Result<()> {
    if let Some(sh) = &h.shorthand {
        let mut chars = sh.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => f.cli.shorthand = Some(c),
            _ => {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!("cli field {key:?}: shorthand must be one character, got {sh:?}"),
                ));
            }
        }
    }
    if h.positional {
        f.cli.positional = true;
    }
    if h.hidden {
        f.cli.hidden = true;
    }
    if h.skip {
        f.cli.skip = true;
    }
    if let Some(ev) = &h.env_var {
        f.cli.env_var = Some(ev.clone());
    }
    if let Some(d) = &h.default {
        f.cli.default = Some(d.clone());
    }
    Ok(())
}

pub fn apply_http_hint(f: &mut FieldMeta, key: &str, h: &HTTPFieldHint) -> errors::Result<()> {
    if !h.location.is_empty() && !crate::spec::field::valid_http_location(&h.location) {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!(
                "http field {key:?}: unknown location {:?} (want query|path|header|form|body)",
                h.location
            ),
        ));
    }
    if !h.location.is_empty() {
        f.http.location = h.location.clone();
    }
    if let Some(n) = &h.name {
        f.http.name = n.clone();
    }
    if let Some(d) = &h.default {
        f.http.default = Some(d.clone());
    }
    Ok(())
}

pub fn apply_mcp_hint(f: &mut FieldMeta, _key: &str, h: &MCPFieldHint) -> errors::Result<()> {
    if let Some(d) = &h.default {
        f.mcp.default = Some(d.clone());
    }
    Ok(())
}
