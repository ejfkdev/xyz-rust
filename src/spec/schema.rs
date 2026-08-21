// Schema 是刻意精简的 JSON Schema（draft-07 味）文档。MCP inputSchema 与
// OpenAPI 生成只需要这个子集；嵌套 struct 内联展开而不是放进 $defs
// （MCP 服务端更乐于接受）。字段序 = 声明序（serde_json preserve_order）。

use serde::Serialize;
use serde_json::{Map, Value};

use crate::spec::field::{FieldKind, FieldMeta};

#[derive(Debug, Clone, Default, Serialize)]
pub struct Schema {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "properties", skip_serializing_if = "Option::is_none")]
    pub properties: Option<Map<String, Value>>,
    #[serde(rename = "items", skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Schema>>,
    #[serde(rename = "required", skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub r#enum: Option<Vec<Value>>,
    #[serde(rename = "default", skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(rename = "format", skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl Schema {
    pub fn object() -> Self {
        Schema {
            r#type: Some("object".to_string()),
            properties: Some(Map::new()),
            ..Default::default()
        }
    }
}

/// Schema -> Value（serde_json 只对 Value 实现 Map 的序列化，properties
/// 存的是属性 schema 的序列化形态）。
pub fn schema_to_value(s: &Schema) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Object(Map::new()))
}

/// 以一组顶层字段构建 object schema（入参根）。
pub fn build_schema(fields: &[FieldMeta]) -> Schema {
    let mut s = Schema::object();
    let mut required = Vec::new();
    let props = s.properties.as_mut().unwrap();
    for f in fields {
        if f.skip {
            continue;
        }
        props.insert(f.json_name.clone(), schema_to_value(&field_schema(f)));
        if f.required {
            required.push(f.json_name.clone());
        }
    }
    if !required.is_empty() {
        s.required = Some(required);
    }
    s
}

/// 单个字段的 schema（Go fieldSchema 的对应物）。
pub fn field_schema(f: &FieldMeta) -> Schema {
    match f.kind {
        FieldKind::Bytes => decorated(
            f,
            Schema {
                r#type: Some("string".to_string()),
                description: opt(f.description.clone()),
                ..Default::default()
            },
        ),
        FieldKind::Duration => decorated(
            f,
            Schema {
                r#type: Some("string".to_string()),
                format: Some("duration".to_string()),
                description: opt(f.description.clone()),
                ..Default::default()
            },
        ),
        FieldKind::Time => decorated(
            f,
            Schema {
                r#type: Some("string".to_string()),
                format: Some("date-time".to_string()),
                description: opt(f.description.clone()),
                ..Default::default()
            },
        ),
        FieldKind::Slice => {
            let mut s = Schema {
                r#type: Some("array".to_string()),
                description: opt(f.description.clone()),
                ..Default::default()
            };
            if let Some(e) = &f.elem {
                s.items = Some(Box::new(field_schema(e)));
            }
            decorated(f, s)
        }
        FieldKind::Struct => {
            let cs = build_schema(&f.children);
            Schema {
                r#type: Some("object".to_string()),
                description: opt(f.description.clone()),
                properties: cs.properties,
                required: cs.required,
                ..Default::default()
            }
        }
        FieldKind::Ptr => {
            // Go 同款：解引用透传元素 schema，再由本层覆盖描述/默认/枚举。
            let mut s = match &f.elem {
                Some(e) => field_schema(e),
                None => Schema::default(),
            };
            if !f.description.is_empty() {
                s.description = Some(f.description.clone());
            }
            if let Some(d) = effective_default(f) {
                s.default = Some(d);
            }
            if !f.enum_values.is_empty() {
                s.r#enum = Some(f.enum_values.clone());
            }
            s
        }
        _ => decorated(
            f,
            Schema {
                r#type: Some(f.kind.schema_type().to_string()),
                description: opt(f.description.clone()),
                ..Default::default()
            },
        ),
    }
}

fn opt(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// 给标量/数组 schema 补 default 与 enum。
fn decorated(f: &FieldMeta, mut s: Schema) -> Schema {
    if let Some(d) = effective_default(f) {
        s.default = Some(d);
    }
    if !f.enum_values.is_empty() {
        s.r#enum = Some(f.enum_values.clone());
    }
    s
}

/// effectiveDefault 交出写进生成 schema 的默认值：MCP 专属覆盖优先于
/// 全局 tag 默认（InputSchema 是 MCP 工具的契约）。
fn effective_default(f: &FieldMeta) -> Option<Value> {
    f.mcp.default.clone().or_else(|| f.default.clone())
}
/// 动态 JSON（Value）不可静态 schematize：None（Go 的接口/map 同款 nil）。
impl crate::spec::XyzSchema for Value {
    fn xyz_schema() -> Option<Schema> {
        None
    }
}
