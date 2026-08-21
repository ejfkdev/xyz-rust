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
        Value::Object(o) => render_kv(w, o),
    }
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
    let width = o.keys().map(|k| k.chars().count()).max().unwrap_or(0);
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
    let mut widths: Vec<usize> = keys.iter().map(|k| k.chars().count()).collect();
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(items.len());
    for item in items {
        let mut row = Vec::with_capacity(keys.len());
        match item.as_object() {
            Some(o) => {
                for (j, k) in keys.iter().enumerate() {
                    let cell = o.get(*k).map(format_cell).unwrap_or_default();
                    widths[j] = widths[j].max(cell.chars().count());
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
