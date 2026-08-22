// 无参数/help 模式的总览输出：三种形态 + 内置参数提示 + 命令表
// （CLI 被禁用时隐藏命令表）。

use std::fmt::Write as _;
use std::io::Write;

use crate::config::Capabilities;
use crate::registry::Registry;

/// 原样输出自定义帮助块：末尾多余换行归一为单个换行，与后续内容自然
/// 分行；空块不输出。
pub fn write_block(w: &mut dyn Write, s: &str) -> std::io::Result<()> {
    if s.is_empty() {
        return Ok(());
    }
    writeln!(w, "{}", s.trim_end_matches('\n'))
}

pub fn print_overview(
    w: &mut dyn Write,
    reg: &Registry,
    serve: &str,
    mcp_word: &str,
    caps: Capabilities,
    help_before: &str,
    help_after: &str,
) -> std::io::Result<()> {
    // before 块直接写目标流；正文攒进 buf。
    write_block(w, help_before)?;
    let mut buf = String::new();
    let _ = writeln!(buf, "{}", crate::lang::t("overview.usage_line"));
    let mut cli_line = crate::lang::t("overview.cli_mode");
    if caps.no_cli {
        cli_line += &crate::lang::t("overview.disabled");
    } else if !crate::dispatch::cli_frontend_compiled() {
        cli_line += &crate::lang::t("overview.not_compiled");
    }
    let _ = writeln!(buf, "{cli_line}");
    let serve_line = crate::lang::tf("overview.serve_mode", &[serve]);
    let serve_line = if caps.no_http {
        serve_line + &crate::lang::t("overview.disabled")
    } else if !crate::dispatch::http_frontend_compiled() {
        serve_line + &crate::lang::t("overview.not_compiled")
    } else {
        serve_line
    };
    let _ = writeln!(buf, "{serve_line}");
    let mcp_line = crate::lang::tf("overview.mcp_mode", &[mcp_word]);
    let mcp_line = if caps.no_mcp {
        mcp_line + &crate::lang::t("overview.disabled")
    } else if !crate::dispatch::mcp_frontend_compiled() {
        mcp_line + &crate::lang::t("overview.not_compiled")
    } else {
        mcp_line
    };
    let _ = writeln!(buf, "{mcp_line}");
    let _ = writeln!(buf, "{}", crate::lang::t("overview.builtins"));
    // CLI 被禁用时不生成子命令，总览也不再列出命令表。
    let names = reg.names();
    if names.is_empty() || caps.no_cli {
        write!(w, "{buf}")?;
        // 自定义 after 块在早退路径照打。
        return write_block(w, help_after);
    }
    let _ = writeln!(buf);
    let _ = writeln!(buf, "{}", crate::lang::t("overview.commands"));
    let width = names.iter().map(|n| n.len()).max().unwrap_or(0);
    for n in &names {
        let summary = reg.get(n).map(|e| e.summary.clone()).unwrap_or_default();
        let _ = writeln!(buf, "  {n:<width$}  {summary}");
    }
    write!(w, "{buf}")?;
    write_block(w, help_after)
}
