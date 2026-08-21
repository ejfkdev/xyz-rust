// 帮助渲染：父节点列子命令、叶子列 flags 与用法；default/env/oneof
// 提示织进描述里。输出文案与 Go 版对齐。

use std::io::Write;

use crate::cli::parse::FlagKind;
use crate::cli::tree::CmdNode;
use crate::errors;
use crate::spec::field::FieldMeta;

pub fn print_help(w: &mut dyn Write, node: &CmdNode, bin: &str) -> errors::Result<()> {
    let desc = if node.long.is_empty() {
        node.short.as_str()
    } else {
        node.long.as_str()
    };
    if !desc.is_empty() {
        writeln!(w, "{desc}")?;
        writeln!(w)?;
    }
    writeln!(w, "Usage:")?;
    write!(w, "  {bin}")?;
    if node.leaf {
        // 自定义 usage 是相对父路径的收尾段（"add <name>"），拼上祖先即可。
        if !node.usage.is_empty() {
            let segs: Vec<&str> = node.path.split('.').collect();
            let prefix = segs[..segs.len().saturating_sub(1)].join(" ");
            if !prefix.is_empty() {
                write!(w, " {prefix}")?;
            }
            write!(w, " {} [flags]", node.usage)?;
        } else {
            write!(w, " {}", node.path.replace('.', " "))?;
            for f in &node.pos_f {
                write!(w, " <{}>", f.json_name)?;
                if !f.required {
                    write!(w, "?")?;
                }
            }
            if !node.defs.is_empty() || node.pos_f.is_empty() {
                write!(w, " [flags]")?;
            }
        }
    } else {
        if !node.path.is_empty() {
            write!(w, " {}", node.path.replace('.', " "))?;
        }
        write!(w, " [命令]")?;
    }
    writeln!(w)?;

    if !node.aliases.is_empty() {
        writeln!(w)?;
        writeln!(w, "Aliases:")?;
        writeln!(w, "  {}", node.aliases.join(", "))?;
    }
    if !node.leaf {
        writeln!(w)?;
        writeln!(w, "命令:")?;
        let visible: Vec<&CmdNode> = node.children.iter().filter(|c| !c.hidden).collect();
        let width = visible
            .iter()
            .map(|c| c.segment.chars().count())
            .max()
            .unwrap_or(0);
        for child in visible {
            writeln!(w, "  {:<width$}  {}", child.segment, child.short)?;
        }
    } else if !node.defs.is_empty() {
        writeln!(w)?;
        writeln!(w, "Flags:")?;
        let mut rows: Vec<(String, String)> = Vec::with_capacity(node.defs.len());
        for d in &node.defs {
            let mut name = format!("--{}", d.long);
            if let Some(sh) = d.short {
                name = format!("-{sh}, {name}");
            }
            let typ = match d.kind {
                FlagKind::Bool => "bool",
                FlagKind::Slice => "strings",
                FlagKind::Str => "string",
            };
            rows.push((format!("{name} {typ}"), flag_description(&d.field)));
        }
        print_rows(w, &rows)?;
    }
    writeln!(w)?;
    writeln!(w, "Global Flags:")?;
    print_rows(
        w,
        &[
            (
                "--json".to_string(),
                "输出 JSON 而不是人类可读格式".to_string(),
            ),
            ("-v, --version".to_string(), "输出版本信息".to_string()),
            ("-h, --help".to_string(), "打印帮助".to_string()),
        ],
    )?;
    Ok(())
}

fn print_rows(w: &mut dyn Write, rows: &[(String, String)]) -> errors::Result<()> {
    let width = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
    for (name, desc) in rows {
        if desc.is_empty() {
            writeln!(w, "  {name}")?;
        } else {
            writeln!(w, "  {name:<width$}  {desc}")?;
        }
    }
    Ok(())
}

/// 把默认值、env 回退与枚举一起织进帮助描述里。
pub fn flag_description(f: &FieldMeta) -> String {
    let mut desc = f.description.clone();
    if let Some(def) = &f.cli.default {
        desc += &format!(" (default {})", display_value(def));
    } else if let Some(def) = &f.default {
        desc += &format!(" (default {})", display_value(def));
    }
    if let Some(env) = &f.cli.env_var {
        desc += &format!(" (env {env})");
    }
    if !f.enum_values.is_empty() {
        let vals = f
            .enum_values
            .iter()
            .map(display_value)
            .collect::<Vec<_>>()
            .join("|");
        desc += &format!(" (oneof {vals})");
    }
    desc
}

fn display_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
