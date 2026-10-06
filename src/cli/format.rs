//! `--format` 的渲染器与取值（spec §10.7）。
//!
//! json 复用既有 --json 路径（pretty），text 走 §9.1 人类渲染；本模块提供
//! jsonl 与 markdown 两个渲染器，以及合法的取值枚举。优先级：显式
//! json/jsonl/markdown 绕过 §9.5 自定义输出，仅 text 进入默认链。

use std::io::Write;

use serde_json::Value;

use crate::errors;

/// 输出格式（spec §10.7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// TTY 感知默认（spec §10.7）：按 writer 是否交互解析为
    /// format_interactive（默认 text）或 format_piped（默认 jsonl）。
    /// resolve 之后不再出现在渲染路径上。
    #[default]
    Auto,
    /// §9.1 人类渲染。
    Text,
    /// pretty JSON（两空格缩进），裸值。
    Json,
    /// JSON Lines：数组逐元素紧凑一行，其余整体一行。
    JsonL,
    /// Markdown：表格/列表渲染。
    Markdown,
}

impl Format {
    /// 解析 `--format` 取值（"" 视为 text）。
    pub fn parse(s: &str) -> Option<Format> {
        match s {
            "" | "auto" => Some(Format::Auto),
            "text" => Some(Format::Text),
            "json" => Some(Format::Json),
            "jsonl" => Some(Format::JsonL),
            "markdown" => Some(Format::Markdown),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Auto => "auto",
            Format::Text => "text",
            Format::Json => "json",
            Format::JsonL => "jsonl",
            Format::Markdown => "markdown",
        }
    }

    /// 机器/备用格式（错误体走 §8.6 对象、绕过自定义渲染的判定用）。
    /// auto 未解析时按非机器处理（解析后不会以 Auto 到达渲染层）。
    pub fn is_machine(&self) -> bool {
        matches!(self, Format::Json | Format::JsonL)
    }

    /// 解析 auto（spec §10.7）：interactive 时取 ifmt（空→text），否则取
    /// pfmt（空→jsonl）；具体值直通。
    pub fn resolve(self, interactive: bool, ifmt: &str, pfmt: &str) -> Format {
        if !matches!(self, Format::Auto) {
            return self;
        }
        let pick = if interactive {
            if ifmt.is_empty() { "text" } else { ifmt }
        } else if pfmt.is_empty() {
            "jsonl"
        } else {
            pfmt
        };
        Format::parse(pick).unwrap_or(if interactive {
            Format::Text
        } else {
            Format::JsonL
        })
    }
}

/// JSON Lines：数组/切片逐元素紧凑一行；其余值整体紧凑一行；null 无输出。
pub fn render_jsonl(w: &mut dyn Write, v: &Value) -> errors::Result<()> {
    match v {
        Value::Null => Ok(()),
        Value::Array(items) => {
            for item in items {
                writeln!(
                    w,
                    "{}",
                    serde_json::to_string(item).unwrap_or_else(|_| "null".into())
                )?;
            }
            Ok(())
        }
        other => {
            writeln!(
                w,
                "{}",
                serde_json::to_string(other).unwrap_or_else(|_| "null".into())
            )?;
            Ok(())
        }
    }
}

/// Markdown（spec §10.7）：
/// null→无输出；标量→裸值；对象→`| Field | Value |` 两列表（插入序，与
/// text 渲染同源；Go 对 map 用 `| Key | Value |` 表头，Rust 侧 struct/map
/// 序列化后同形，统一 Field 表头——见 README 差异节）；
/// 对象数组→以首元素键为列的表；标量数组→`- item` 子弹；单元格转义
/// `|`→`\|`、换行→`<br>`。
pub fn render_markdown(w: &mut dyn Write, v: &Value) -> errors::Result<()> {
    match v {
        Value::Null => Ok(()),
        Value::String(_) | Value::Bool(_) | Value::Number(_) => {
            writeln!(w, "{}", md_escape(&cell(v)))?;
            Ok(())
        }
        Value::Object(o) => md_kv(w, o),
        Value::Array(items) => {
            if items.is_empty() {
                return Ok(());
            }
            if items[0].is_object() {
                md_table(w, items)
            } else {
                for item in items {
                    writeln!(w, "- {}", md_escape(&cell(item)))?;
                }
                Ok(())
            }
        }
    }
}

fn md_kv(w: &mut dyn Write, o: &serde_json::Map<String, Value>) -> errors::Result<()> {
    if o.is_empty() {
        return Ok(());
    }
    writeln!(w, "| Field | Value |")?;
    writeln!(w, "| --- | --- |")?;
    for (k, val) in o {
        writeln!(w, "| {} | {} |", md_escape(k), md_escape(&cell(val)))?;
    }
    Ok(())
}

fn md_table(w: &mut dyn Write, items: &[Value]) -> errors::Result<()> {
    let Some(first) = items[0].as_object() else {
        return Ok(());
    };
    let keys: Vec<&String> = first.keys().collect();
    if keys.is_empty() {
        return Ok(());
    }
    write!(w, "|")?;
    for k in &keys {
        write!(w, " {} |", md_escape(k))?;
    }
    writeln!(w)?;
    write!(w, "|")?;
    for _ in &keys {
        write!(w, " --- |")?;
    }
    writeln!(w)?;
    for item in items {
        write!(w, "|")?;
        let row = item.as_object();
        for k in &keys {
            let val = row.and_then(|r| r.get(k.as_str()));
            let text = match val {
                Some(v) => md_escape(&cell(v)),
                None => String::new(),
            };
            write!(w, " {text} |")?;
        }
        writeln!(w)?;
    }
    Ok(())
}

/// 单元格字符串：复用 §9.1 的单值渲染（数组 → `[a b]`、对象 → JSON 等）。
fn cell(v: &Value) -> String {
    crate::cli::render::format_cell(v)
}

/// 转义会破坏 Markdown 表格的字符：竖线与换行。
fn md_escape(s: &str) -> String {
    s.replace("\r\n", "<br>")
        .replace('\n', "<br>")
        .replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn md(v: &Value) -> String {
        let mut buf = Vec::new();
        render_markdown(&mut buf, v).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn jsonl(v: &Value) -> String {
        let mut buf = Vec::new();
        render_jsonl(&mut buf, v).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn jsonl_shapes() {
        assert_eq!(jsonl(&json!(null)), "");
        assert_eq!(jsonl(&json!([1, 2])), "1\n2\n");
        assert_eq!(jsonl(&json!({"a": 1})), "{\"a\":1}\n");
        assert_eq!(jsonl(&json!("x")), "\"x\"\n");
    }

    #[test]
    fn markdown_shapes() {
        assert_eq!(md(&json!(null)), "");
        assert_eq!(md(&json!("hi")), "hi\n");
        assert_eq!(
            md(&json!({"k": "v", "long": 1})),
            "| Field | Value |\n| --- | --- |\n| k | v |\n| long | 1 |\n"
        );
        assert_eq!(md(&json!([1, "a"])), "- 1\n- a\n");
        assert_eq!(
            md(&json!([{"a": 1, "b": 2}, {"a": 3, "b": 4}])),
            "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n"
        );
        // 转义：竖线与换行不破表。
        assert_eq!(
            md(&json!({"k": "a|b\nc"})),
            "| Field | Value |\n| --- | --- |\n| k | a\\|b<br>c |\n"
        );
        // 空对象/空数组无输出。
        assert_eq!(md(&json!({})), "");
        assert_eq!(md(&json!([])), "");
    }

    #[test]
    fn format_parsing() {
        assert_eq!(Format::parse(""), Some(Format::Auto));
        assert_eq!(Format::parse("auto"), Some(Format::Auto));
        assert_eq!(Format::parse("jsonl"), Some(Format::JsonL));
        assert_eq!(Format::parse("MARKDOWN"), None);
        assert!(Format::Json.is_machine() && !Format::Markdown.is_machine());
    }
}
