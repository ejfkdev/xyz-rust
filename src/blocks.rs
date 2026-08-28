//! 内容块结果（spec §12.7）：handler 可以返回 `Blocks` 而不是纯 JSON 值。
//!
//! 块结果穿过三个前端前先被序列化成**保留块信封**——唯一键为 `content`
//! 的对象、数组项形如 `{"type":"text","text":…}` 或
//! `{"type":"image","mimeType":…,"data":…}`（载荷 base64）——前端凭形状把它
//! 从普通 JSON 里识别出来（[`extract`]）。MCP 原样透传为 `Content`；CLI 把
//! 二进制块落盘到临时目录并打印路径；HTTP 直出信封 JSON。
//!
//! 载荷不经过库编码/截断：调用方负责提供 base64 字符串。

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;

use crate::errors;
use crate::spec::schema::Schema;
use crate::spec::XyzSchema;

/// 单块内容。与 MCP `Content` 一 一对应（P0 支持 text/image/audio；
/// resource 留待需要时加入）。
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text { text: String },
    /// `data` 是 base64 载荷（MCP 协议形态，不做原始字节存储）。
    Image { mime_type: String, data: String },
    Audio { mime_type: String, data: String },
}

/// 块结果：handler 的返回类型（实现 [`XyzSchema`] 与 [`Serialize`]，
/// 直接满足 `define` 的结果约束）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Blocks {
    pub content: Vec<Block>,
}

impl Blocks {
    pub fn new(content: Vec<Block>) -> Self {
        Blocks { content }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Blocks::new(vec![Block::Text { text: text.into() }])
    }

    pub fn image(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Blocks::new(vec![Block::Image {
            mime_type: mime_type.into(),
            data: data.into(),
        }])
    }

    pub fn audio(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Blocks::new(vec![Block::Audio {
            mime_type: mime_type.into(),
            data: data.into(),
        }])
    }

    /// 追加一块。
    pub fn push(&mut self, block: Block) {
        self.content.push(block);
    }
}

impl Serialize for Block {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(3))?;
        match self {
            Block::Text { text } => {
                map.serialize_entry("type", "text")?;
                map.serialize_entry("text", text)?;
            }
            Block::Image { mime_type, data } | Block::Audio { mime_type, data } => {
                let kind = if matches!(self, Block::Image { .. }) {
                    "image"
                } else {
                    "audio"
                };
                map.serialize_entry("type", kind)?;
                map.serialize_entry("mimeType", mime_type)?;
                map.serialize_entry("data", data)?;
            }
        }
        map.end()
    }
}

impl Serialize for Blocks {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(1))?;
        map.serialize_entry("content", &self.content)?;
        map.end()
    }
}

impl XyzSchema for Blocks {
    fn xyz_schema() -> Option<Schema> {
        // 块结果没有单一 JSON Schema：outputSchema 缺省（前端按信封形状渲染）。
        None
    }
}

/// 判定保留块信封并还原块清单。普通 JSON 返回 `None`；形状不完全吻合也
/// 整体返回 `None`（部分匹配拒绝，绝不半渲染半透传）。
pub fn extract(v: &Value) -> Option<Vec<Block>> {
    let obj = v.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    let items = obj.get("content")?.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(block_from_value(item)?);
    }
    Some(out)
}

fn block_from_value(v: &Value) -> Option<Block> {
    let o = v.as_object()?;
    match o.get("type")?.as_str()? {
        "text" => Some(Block::Text {
            text: o.get("text")?.as_str()?.to_string(),
        }),
        "image" => Some(Block::Image {
            mime_type: o.get("mimeType")?.as_str()?.to_string(),
            data: o.get("data")?.as_str()?.to_string(),
        }),
        "audio" => Some(Block::Audio {
            mime_type: o.get("mimeType")?.as_str()?.to_string(),
            data: o.get("data")?.as_str()?.to_string(),
        }),
        _ => None,
    }
}

/// MIME → 文件扩展名（CLI 落盘时用；未知类型统一 `bin`）。
pub fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "audio/ogg" => "ogg",
        _ => "bin",
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 解码标准 base64（可选 `=` padding，容忍空白）。库零三方依赖：这是
/// 约 40 行的标准算法，只为 CLI 落盘与信封形状校验服务。
pub fn decode_base64(s: &str) -> errors::Result<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        B64.iter().position(|&b| b == c).map(|i| i as u32)
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u8;
    for b in s.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            break; // padding 截止；余下交给长度检查
        }
        let Some(v) = val(b) else {
            return Err(errors::new(
                errors::Kind::Internal,
                format!("invalid base64 payload: byte {b:#04x}"),
            ));
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_round_trip() {
        let b = Blocks::new(vec![
            Block::Text {
                text: "hello".to_string(),
            },
            Block::Image {
                mime_type: "image/png".into(),
                data: "aGVsbG8=".into(),
            },
        ]);
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(
            v,
            json!({"content":[
                {"type":"text","text":"hello"},
                {"type":"image","mimeType":"image/png","data":"aGVsbG8="}
            ]})
        );
        let back = extract(&v).unwrap();
        assert_eq!(back, b.content);
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVs bG8=").unwrap(), b"hello"); // 容忍空白
        assert!(decode_base64("not*b64").is_err());
    }

    #[test]
    fn extract_rejects_non_envelopes() {
        // 普通对象、多键对象、坏形状项、未知类型＝全部按普通 JSON 处理。
        assert!(extract(&json!({"msg": "hi"})).is_none());
        assert!(extract(&json!({"content": "text"})).is_none());
        assert!(extract(&json!({"content": [{"type": "text"}]})).is_none());
        assert!(extract(&json!({"content": [{"type": "video", "data": "x"}]})).is_none());
        assert!(extract(&json!({"content": [{"type": "text", "text": "a"}]})).is_some());
        assert!(extract(&json!({"content": [], "extra": 1})).is_none());
        // 空 content 数组是合法空信封。
        assert_eq!(extract(&json!({"content": []})), Some(vec![]));
    }

    #[test]
    fn mime_extensions() {
        assert_eq!(ext_for_mime("image/png"), "png");
        assert_eq!(ext_for_mime("audio/mpeg"), "mp3");
        assert_eq!(ext_for_mime("application/x-unknown"), "bin");
    }
}