// 结果渲染（Go cli.Render 的对应物）：
//
//	null                 -> 什么都不输出
//	string               -> 原始字符串
//	bool / 数字          -> 裸值
//	数组（对象元素）      -> 对齐表格（表头 + 分隔线 + 行）
//	数组（标量元素）      -> 每行一个
//	对象（struct 或 map） -> 对齐 "key  value" 列
//
// 结果从不包 {"data": ...} 信封：--json 直接输出裸 JSON 值而不是走
// Render。与 Go 版的差异（已记录在 README 差异节）：
//   - Rust 侧渲染输入是序列化后的 serde_json::Value——struct 与
//     同形 map 都表现为 Object，按 serde 产出顺序输出（Go 的 map 会
//     按键排序；struct 保持声明序，preserve_order 已对齐后者）；
//   - 浮点裸值按 f64 Display 输出（"3" 而非 "3.0"），对齐 Go 的 %v。

use std::io::Write;

use serde::Serialize;
use serde_json::Value;

use crate::errors;

/// 把可序列化结果渲染成人类可读形态。
pub fn render<R: Serialize>(w: &mut dyn Write, v: &R) -> errors::Result<()> {
    let value = serde_json::to_value(v).map_err(|e| {
        errors::Error::new(errors::Kind::Internal, format!("result serialization: {e}"))
    })?;
    render_value(w, &value)
}

/// 直接渲染一个 Value（Entry.invoke 的产物形态）。
pub fn render_value(w: &mut dyn Write, v: &Value) -> errors::Result<()> {
    match v {
        Value::Null => Ok(()),
        Value::String(s) => {
            writeln!(w, "{s}")?;
            Ok(())
        }
        Value::Bool(b) => {
            writeln!(w, "{b}")?;
            Ok(())
        }
        Value::Number(n) => {
            writeln!(w, "{}", fmt_number(n))?;
            Ok(())
        }
        Value::Array(items) => {
            if items.is_empty() {
                return Ok(());
            }
            if items.first().map(Value::is_object).unwrap_or(false) {
                render_table(w, items)
            } else {
                for item in items {
                    writeln!(w, "{}", format_cell(item))?;
                }
                Ok(())
            }
        }
        Value::Object(o) => {
            // 保留块信封（spec §12.7）：文本块内联、二进制块落盘打路径。
            if let Some(blocks) = crate::blocks::extract(v) {
                return render_blocks(w, &blocks);
            }
            render_kv(w, o)
        }
    }
}

/// 渲染块清单：text 直出；image/audio 落盘临时目录、打印文件路径——
/// 大载荷不进终端（spec §12.7 的 CLI 投影）。
fn render_blocks(w: &mut dyn Write, blocks: &[crate::blocks::Block]) -> errors::Result<()> {
    for b in blocks {
        match b {
            crate::blocks::Block::Text { text } => writeln!(w, "{text}")?,
            crate::blocks::Block::Image { mime_type, data }
            | crate::blocks::Block::Audio { mime_type, data } => {
                let path = spill_to(&std::env::temp_dir(), mime_type, data)?;
                writeln!(w, "{}", path.display())?;
            }
        }
    }
    Ok(())
}

/// 把一块 base64 载荷落盘：文件名 xyz-blk-<pid>-<seq>.<ext>。
pub fn spill_to(
    dir: &std::path::Path,
    mime_type: &str,
    data: &str,
) -> errors::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let bytes = crate::blocks::decode_base64(data)?;
    let ext = crate::blocks::ext_for_mime(mime_type);
    let name = format!(
        "xyz-blk-{}-{}.{ext}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn fmt_number(n: &serde_json::Number) -> String {
    // f64 走 Display：与 Go %v 一致的裸浮点形态（"0.25"、"3"）。
    if let Some(f) = n.as_f64()
        && n.as_i64().is_none()
        && n.as_u64().is_none()
    {
        return f.to_string();
    }
    n.to_string()
}

fn render_kv(w: &mut dyn Write, o: &serde_json::Map<String, Value>) -> errors::Result<()> {
    if o.is_empty() {
        return Ok(());
    }
    // 宽度按字节计（Go 的 len()），填充按字符计（Go 的 %-*s）——两者合一
    // 才能与 Go 的 CJK 对齐逐字节一致。
    let width = o.keys().map(|k| k.len()).max().unwrap_or(0);
    for (k, v) in o {
        writeln!(w, "{k:<width$}  {}", format_cell(v))?;
    }
    Ok(())
}

fn render_table(w: &mut dyn Write, items: &[Value]) -> errors::Result<()> {
    let first = match items.first().and_then(Value::as_object) {
        Some(o) => o,
        None => return Ok(()),
    };
    let keys: Vec<&String> = first.keys().collect();
    if keys.is_empty() {
        return Ok(());
    }
    // 宽度按字符计（fmt 的 `{:<width$}` 以字符补白），并对补白宽度封顶：
    // Rust 的命名宽度参数是 u16（> u16::MAX 会 panic! "Formatting argument
    // out of range"），超长单元格（引用方法列表等）曾直接打死渲染器；封顶
    // 同时避免按全局最宽单元格把每行都补到几十 KB。
    const MAX_PAD: usize = 200;
    let cell_width = |c: &str| c.chars().count().min(MAX_PAD);
    let mut widths: Vec<usize> = keys.iter().map(|k| cell_width(k)).collect();
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(items.len());
    for item in items {
        let mut row = Vec::with_capacity(keys.len());
        match item.as_object() {
            Some(o) => {
                for (j, k) in keys.iter().enumerate() {
                    let cell = o.get(*k).map(format_cell).unwrap_or_default();
                    widths[j] = widths[j].max(cell_width(&cell));
                    row.push(cell);
                }
            }
            None => {
                for k in keys.iter() {
                    row.push(format_cell(item));
                    let _ = k;
                }
            }
        }
        rows.push(row);
    }
    write_row(
        w,
        &keys.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
        &widths,
    )?;
    let dashes: Vec<String> = widths.iter().map(|wd| "-".repeat(*wd)).collect();
    write_row(w, &dashes, &widths)?;
    for row in &rows {
        write_row(w, row, &widths)?;
    }
    Ok(())
}

fn write_row(w: &mut dyn Write, cells: &[String], widths: &[usize]) -> errors::Result<()> {
    for (i, c) in cells.iter().enumerate() {
        if i > 0 {
            write!(w, "  ")?;
        }
        write!(w, "{c:<width$}", width = widths[i])?;
    }
    writeln!(w)?;
    Ok(())
}

/// 单元格/键值取值：指针解引用（JSON 里不存在指针对象）、切片括号
/// 连接，其余按裸值。
pub fn format_cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => fmt_number(n),
        Value::Array(items) => {
            let inner = items.iter().map(format_cell).collect::<Vec<_>>().join(" ");
            format!("[{inner}]")
        }
        Value::Object(o) => serde_json::to_string(o).unwrap_or_else(|_| "{}".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(v: &Value) -> String {
        let mut out = Vec::new();
        render_value(&mut out, v).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn long_cells_do_not_panic_and_pad_is_capped() {
        // 70k chars > u16::MAX (the width argument limit) — used to panic.
        let long = "x".repeat(70_000);
        let v = serde_json::json!([
            {"a": long.clone(), "b": "y"},
            {"a": "short", "b": "z"},
        ]);
        let text = render(&v);
        assert!(text.contains(&long), "long cell content must be preserved");
        // The padding width is capped: the short row must not carry 70k spaces.
        let second = text.lines().nth(3).unwrap();
        assert!(
            second.len() < 1000,
            "row padded to the long cell width: {} bytes",
            second.len()
        );
    }

    #[test]
    fn cjk_cells_use_char_widths() {
        let v = serde_json::json!([
            {"name": "中文", "n": 1},
            {"name": "ab", "n": 2},
        ]);
        let text = render(&v);
        // header rule width == char count of the widest cell (2), not bytes.
        let rule = text.lines().nth(1).unwrap();
        assert!(rule.starts_with("----"), "{rule}");
        assert_eq!(rule.split("  ").next().unwrap().chars().count(), 4);
    }
}
