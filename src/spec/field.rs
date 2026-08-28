// FieldSpec 是宏生成的静态字段描述（tag 原串 + 类型形状）；FieldMeta 是
// 注册期解析出的运行时元数据（typed 默认值、枚举、校验规则）。两者分离
// 对应 Go 的「tag 原串在 reflect 里、解析在 Entry 构建期」。
//
// 嵌套 struct 的字段树由宏递归生成（XyzField::xyz_spec_of），无需任何
// 运行时类型分析；递归类型在注册期经深度护栏报错（Go 的 maxAnalyzeDepth
// 对应物）。

use serde_json::Value;

use crate::errors;
use crate::spec::validate::{self, VRule};

/// 类型形状（Go reflect.Kind 的对齐物，按 xyz 关心的子集收窄）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldKind {
    #[default]
    String,
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    /// std::time::Duration（Go time.Duration）。
    Duration,
    /// chrono::DateTime<Utc>（Go time.Time）。
    Time,
    /// Vec<u8>（Go []byte）。
    Bytes,
    /// Option<T>（Go *T）。
    Ptr,
    /// Vec<T>（Go []T）。
    Slice,
    /// 嵌套 struct / 命名 newtype 的归宿。
    Struct,
    /// spec §4.7 邻接带标签联合（enum 字段）；变体树放 FieldMeta.union。
    Union,
}

impl FieldKind {
    /// JSON Schema 基础类型（OpenAPI 参数表与 schema 生成共用）。
    pub fn schema_type(&self) -> &'static str {
        match self {
            FieldKind::String | FieldKind::Duration | FieldKind::Time | FieldKind::Bytes => {
                "string"
            }
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
            FieldKind::Ptr => "pointer",
            FieldKind::Struct | FieldKind::Union => "object",
        }
    }

    pub fn is_scalar(&self) -> bool {
        matches!(
            self,
            FieldKind::String
                | FieldKind::Bool
                | FieldKind::I8
                | FieldKind::I16
                | FieldKind::I32
                | FieldKind::I64
                | FieldKind::U8
                | FieldKind::U16
                | FieldKind::U32
                | FieldKind::U64
                | FieldKind::F32
                | FieldKind::F64
        )
    }

    pub fn is_numeric(&self) -> bool {
        self.is_scalar() && !matches!(self, FieldKind::String | FieldKind::Bool)
    }
}

/// 联合的静态树（宏在注册链路上构建；tag/name 是 'static 字面量，
/// 字段树用 owned 容器——FieldSpec 含 Vec 不可 const，宏以 vec! 生成）。
#[derive(Debug, Clone)]
pub struct UnionSpec {
    /// serde `#[serde(tag = "…")]` 的判别键。
    pub tag: &'static str,
    /// 各变体的字段树。
    pub variants: Vec<UnionVariantSpec>,
}

#[derive(Debug, Clone)]
pub struct UnionVariantSpec {
    pub name: &'static str,
    pub fields: Vec<FieldSpec>,
}

/// 联合的运行时树（FieldSpec 解析产物）。
#[derive(Debug, Clone)]
pub struct UnionMeta {
    pub tag: String,
    pub variants: Vec<UnionVariant>,
}

#[derive(Debug, Clone)]
pub struct UnionVariant {
    pub name: String,
    /// 变体字段的运行时元数据（嵌套校验/解码用）。
    pub meta: Vec<FieldMeta>,
}

/// 静态字段描述（宏按声明序生成的字段树节点）。
#[derive(Debug, Clone)]
pub struct FieldSpec {
    /// Rust 字段名（json:"-" 注入字段的投递键）。
    pub rust_name: &'static str,
    /// 线上名。
    pub json_name: &'static str,
    pub kind: FieldKind,
    pub desc: &'static str,
    pub required: bool,
    pub secret: bool,
    /// json:"-" 对应物：不进绑定与 schema（env/header 仍可按 rust_name 注入）。
    pub skip: bool,
    /// validate tag 原串（注册期解析）。
    pub validate_s: &'static str,
    /// enum tag 原串（注册期解析）。
    pub enum_s: &'static str,
    /// default tag 原串（注册期按 kind 解析成 typed Value）。
    pub default_s: &'static str,
    /// cli tag 原串（register 期解析：shorthand/positional/hidden/env/-）。
    pub cli_s: &'static str,
    /// http location 原串（query/path/header/form/body/空）。
    pub http_s: &'static str,
    /// httpName 覆盖（header 名等）。
    pub http_name: Option<&'static str>,
    /// Ptr/Slice 的元素形状。
    pub elem: Option<Box<FieldSpec>>,
    /// struct 的子字段。
    pub children: Vec<FieldSpec>,
    /// 联合的静态树（kind == Union 时填充）。
    pub union: Option<UnionSpec>,
}

/// 宏生成结构的元素/裸类型节点（无名字的合成包装）。
pub fn synthetic(
    kind: FieldKind,
    children: Vec<FieldSpec>,
    elem: Option<Box<FieldSpec>>,
) -> FieldSpec {
    FieldSpec {
        rust_name: "",
        json_name: "",
        kind,
        desc: "",
        required: false,
        secret: false,
        skip: false,
        validate_s: "",
        enum_s: "",
        default_s: "",
        cli_s: "",
        http_s: "",
        http_name: None,
        elem,
        children,
        union: None,
    }
}

/// 运行时字段的 CLI 绑定（cli tag / CliFieldHint 合并后的产物）。
#[derive(Debug, Clone, Default)]
pub struct CliField {
    pub shorthand: Option<char>,
    pub positional: bool,
    pub hidden: bool,
    pub skip: bool,
    pub env_var: Option<String>,
    /// CLI-only 默认值；覆盖全局 default tag（对 CLI 前端生效）。
    pub default: Option<Value>,
}

/// 运行时字段的 HTTP 绑定。
#[derive(Debug, Clone, Default)]
pub struct HTTPField {
    /// ""（未标注，默认 query）| query | path | header | form | body。
    pub location: String,
    pub name: String,
    pub default: Option<Value>,
}

/// 运行时字段的 MCP 绑定。MCP 的覆盖同时替换 inputSchema 里的 default。
#[derive(Debug, Clone, Default)]
pub struct MCPField {
    pub default: Option<Value>,
}

/// 运行时字段元数据（FieldSpec 解析 + Define-time hints 合并的产物）。
#[derive(Debug, Clone, Default)]
pub struct FieldMeta {
    pub name: String,
    pub json_name: String,
    pub kind: FieldKind,
    pub description: String,
    pub required: bool,
    pub secret: bool,
    pub validate: String,
    pub rules: Vec<VRule>,
    /// 已解析的 typed 枚举值（仅标量）。
    pub enum_values: Vec<Value>,
    /// 已解析的 typed 全局默认；None 表示无默认。
    pub default: Option<Value>,
    pub skip: bool,
    pub cli: CliField,
    pub http: HTTPField,
    pub mcp: MCPField,
    /// Ptr/Slice 的元素元数据。
    pub elem: Option<Box<FieldMeta>>,
    /// struct 的子字段。
    pub children: Vec<FieldMeta>,
    /// 联合的运行时树（kind == Union 时填充；变体字段表）。
    pub union: Option<UnionMeta>,
}

/// 宏内自引用的护栏：递归类型（Node 里套 Node）在生成字段树时栈溢出前
/// 报错。Entry 构建用 catch_unwind 把它转成注册期错误。
pub const MAX_SPEC_DEPTH: usize = 20;

thread_local! {
    static SPEC_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 由宏生成的 xyz_spec() 入口调用；超深时报错（注册期被捕获）。
pub fn spec_depth_guard() {
    SPEC_DEPTH.with(|d| {
        let cur = d.get();
        if cur > MAX_SPEC_DEPTH {
            panic!(
                "xyz: argument struct nesting exceeds {} levels (recursive type?)",
                MAX_SPEC_DEPTH
            );
        }
        d.set(cur + 1);
    })
}

/// 解析 FieldSpec 树为运行时 FieldMeta 树。所有 tag 解析错误都在这里
/// 发生——注册期即报错，而不是运行时悄悄忽略。
pub fn meta_from_spec(spec: &FieldSpec) -> errors::Result<FieldMeta> {
    let cli = parse_cli_tag(spec.cli_s)?;
    let http = parse_http_tag(spec.http_s, spec.http_name)?;
    let rules = validate::parse_validate_tag(spec.validate_s)?;
    let default = if spec.default_s.is_empty() {
        None
    } else {
        Some(parse_default(spec, spec.default_s)?)
    };
    let enum_values = if spec.enum_s.is_empty() {
        Vec::new()
    } else {
        parse_enum(spec.kind, spec.enum_s)?
    };
    let elem = match &spec.elem {
        Some(e) => Some(Box::new(meta_from_spec(e)?)),
        None => None,
    };
    let children = spec
        .children
        .iter()
        .map(meta_from_spec)
        .collect::<errors::Result<Vec<_>>>()?;
    let union = match &spec.union {
        Some(u) => Some(UnionMeta {
            tag: u.tag.to_string(),
            variants: u
                .variants
                .iter()
                .map(|v| {
                    Ok(UnionVariant {
                        name: v.name.to_string(),
                        meta: v
                            .fields
                            .iter()
                            .map(meta_from_spec)
                            .collect::<errors::Result<Vec<_>>>()?,
                    })
                })
                .collect::<errors::Result<Vec<_>>>()?,
        }),
        None => None,
    };
    Ok(FieldMeta {
        name: spec.rust_name.to_string(),
        json_name: spec.json_name.to_string(),
        kind: spec.kind,
        description: spec.desc.to_string(),
        required: spec.required,
        secret: spec.secret,
        validate: spec.validate_s.to_string(),
        rules,
        enum_values,
        default,
        skip: spec.skip,
        cli,
        http,
        mcp: MCPField::default(),
        elem,
        children,
        union,
    })
}

/// 解析 cli tag 原串：shorthand=a, positional, hidden, env=VAR, -。
pub fn parse_cli_tag(s: &str) -> errors::Result<CliField> {
    let mut f = CliField::default();
    if s.trim() == "-" {
        f.skip = true;
        return Ok(f);
    }
    for tok in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if tok == "positional" {
            f.positional = true;
        } else if tok == "hidden" {
            f.hidden = true;
        } else if let Some(sh) = tok.strip_prefix("shorthand=") {
            let mut chars = sh.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => f.shorthand = Some(c),
                _ => {
                    return Err(errors::Error::new(
                        errors::Kind::Internal,
                        format!("cli shorthand must be one character, got {sh:?}"),
                    ));
                }
            }
        } else if let Some(ev) = tok.strip_prefix("env=") {
            if ev.is_empty() {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    "cli env requires a variable name".to_string(),
                ));
            }
            f.env_var = Some(ev.to_string());
        } else {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!("unknown cli option {tok:?} (want positional|hidden|shorthand=X|env=X|-)"),
            ));
        }
    }
    Ok(f)
}

/// 解析 http location（tag 或 hint 共用的校验入口）。
pub fn valid_http_location(s: &str) -> bool {
    matches!(s, "" | "query" | "path" | "header" | "form" | "body")
}

pub fn parse_http_tag(s: &str, http_name: Option<&str>) -> errors::Result<HTTPField> {
    if !valid_http_location(s) {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!("unknown http location {s:?} (want query|path|header|form|body)"),
        ));
    }
    Ok(HTTPField {
        location: s.to_string(),
        name: http_name.unwrap_or_default().to_string(),
        default: None,
    })
}

/// 按类型形状把 default tag 原串解析成 typed Value（Go parseDefault 对应物：
/// 复用与 decodeValue 相同的转换语义，Duration/Time 在注册期先验语法）。
pub fn parse_default(spec: &FieldSpec, s: &str) -> errors::Result<Value> {
    match spec.kind {
        FieldKind::Duration => {
            crate::spec::scalar::parse_duration(s).map_err(|e| {
                errors::Error::new(errors::Kind::Internal, format!("bad default {s:?}: {e}"))
            })?;
            Ok(Value::String(s.to_string()))
        }
        FieldKind::Time => {
            crate::spec::scalar::parse_datetime(s).map_err(|e| {
                errors::Error::new(errors::Kind::Internal, format!("bad default {s:?}: {e}"))
            })?;
            Ok(Value::String(s.to_string()))
        }
        FieldKind::Bytes => Ok(Value::String(s.to_string())),
        FieldKind::Slice => {
            let elem_kind = spec.elem.as_ref().map(|e| e.kind).ok_or_else(|| {
                errors::Error::new(
                    errors::Kind::Internal,
                    "slice without elem kind".to_string(),
                )
            })?;
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| {
                    crate::spec::scalar::parse_scalar_literal(elem_kind, p).map_err(|e| {
                        errors::Error::new(errors::Kind::Internal, format!("element {p:?}: {e}"))
                    })
                })
                .collect()
        }
        FieldKind::Ptr => match &spec.elem {
            Some(e) => parse_default(e, s),
            None => Err(errors::Error::new(
                errors::Kind::Internal,
                "pointer without elem kind".to_string(),
            )),
        },
        FieldKind::Struct => Err(errors::Error::new(
            errors::Kind::Internal,
            format!("unsupported scalar kind Struct for default {s:?}"),
        )),
        _ => crate::spec::scalar::parse_scalar_literal(spec.kind, s).map_err(|e| {
            errors::Error::new(errors::Kind::Internal, format!("bad default {s:?}: {e}"))
        }),
    }
}

/// 按 kind 把 enum tag 原串解析成 typed Value 列表。仅标量支持枚举
/// （Go parseEnum 同一约束：Ptr/Struct/Slice/Duration/Time 报错）。
pub fn parse_enum(kind: FieldKind, s: &str) -> errors::Result<Vec<Value>> {
    if !matches!(
        kind,
        FieldKind::String
            | FieldKind::Bool
            | FieldKind::I8
            | FieldKind::I16
            | FieldKind::I32
            | FieldKind::I64
            | FieldKind::U8
            | FieldKind::U16
            | FieldKind::U32
            | FieldKind::U64
            | FieldKind::F32
            | FieldKind::F64
    ) {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!("enum is only supported on scalar fields, got {kind:?}"),
        ));
    }
    let mut out = Vec::new();
    for p in s.split(',').map(str::trim) {
        if p.is_empty() {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                "enum contains an empty value".to_string(),
            ));
        }
        let v = crate::spec::scalar::parse_scalar_literal(kind, p)?;
        out.push(v);
    }
    Ok(out)
}
