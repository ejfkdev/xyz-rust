// 帮助渲染：父节点列子命令、叶子列 flags 与用法；default/env/oneof
// 提示织进描述里。输出文案与 Go 版对齐。

use std::io::Write;

use crate::cli::tree::CmdNode;
use crate::errors;
use crate::spec::field::FieldMeta;

pub fn print_help(w: &mut dyn Write, node: &CmdNode, bin: &str) -> errors::Result<()> {
    // 自定义块：只在叶子命令上生效（中间节点没有 CliHints）。
    if node.leaf
        && let Some(entry) = &node.entry
    {
        write_help_block(w, &entry.cli.before)?;
    }
    let desc = if node.long.is_empty() {
        node.short.as_str()
    } else {
        node.long.as_str()
    };
    if !desc.is_empty() {
        writeln!(w, "{desc}")?;
        writeln!(w)?;
    }
    writeln!(w, "{}", crate::lang::t("help.usage"))?;
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
        write!(w, " {}", crate::lang::t("help.commands_placeholder"))?;
    }
    writeln!(w)?;

    if !node.aliases.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", crate::lang::t("help.aliases"))?;
        writeln!(w, "  {}", node.aliases.join(", "))?;
    }
    if !node.leaf {
        writeln!(w)?;
        writeln!(w, "{}", crate::lang::t("help.commands"))?;
        let visible: Vec<&CmdNode> = node.children.iter().filter(|c| !c.hidden).collect();
        let width = visible.iter().map(|c| c.segment.len()).max().unwrap_or(0);
        for child in visible {
            writeln!(w, "  {:<width$}  {}", child.segment, child.short)?;
        }
    } else if !node.defs.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", crate::lang::t("help.flags"))?;
        let mut rows: Vec<(String, String)> = Vec::with_capacity(node.defs.len());
        for d in &node.defs {
            let mut name = format!("--{}", d.long);
            if let Some(sh) = d.short {
                name = format!("-{sh}, {name}");
            }
            let typ = flag_type_name(d);
            rows.push((format!("{name} {typ}"), flag_description(&d.field)));
        }
        print_rows(w, &rows)?;
    }
    writeln!(w)?;
    writeln!(w, "{}", crate::lang::t("help.global_flags"))?;
    print_rows(
        w,
        &[
            ("--json".to_string(), crate::lang::t("help.json_flag")),
            (
                "-v, --version".to_string(),
                crate::lang::t("help.version_flag"),
            ),
            ("-h, --help".to_string(), crate::lang::t("help.help_flag")),
        ],
    )?;
    if node.leaf
        && let Some(entry) = &node.entry
    {
        write_help_block(w, &entry.cli.after)?;
    }
    Ok(())
}

/// 原样输出 -h 的自定义文本块：末尾换行归一；空块不输出。
pub fn write_help_block(w: &mut dyn Write, s: &str) -> errors::Result<()> {
    if s.is_empty() {
        return Ok(());
    }
    writeln!(w, "{}", s.trim_end_matches('\n')).map_err(errors::Error::from)
}

/// 按字段类型保真渲染帮助里的 flag 类型；切片同时标注可重复。
fn flag_type_name(d: &crate::cli::parse::FlagDef) -> &'static str {
    use crate::spec::field::FieldKind as K;
    match d.kind {
        crate::cli::parse::FlagKind::Bool => "bool",
        crate::cli::parse::FlagKind::Slice => "strings (repeatable)",
        crate::cli::parse::FlagKind::Str => match d.field.kind {
            K::I8 | K::I16 | K::I32 | K::I64 | K::U8 | K::U16 | K::U32 | K::U64 => "integer",
            K::F32 | K::F64 => "number",
            K::Duration => "duration",
            K::Time => "time",
            _ => "string",
        },
    }
}

fn print_rows(w: &mut dyn Write, rows: &[(String, String)]) -> errors::Result<()> {
    let width = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
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
