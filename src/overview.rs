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
    let _ = writeln!(buf, "用法（模式由程序自动判断，定义只有一份）:");
    let mut cli_line =
        "  <app> [命令] [参数]           CLI 模式（子命令 + flag/位置参数；-h 帮助，-v 版本）"
            .to_string();
    if caps.no_cli {
        cli_line += "（已禁用）";
    } else if !crate::dispatch::cli_frontend_compiled() {
        cli_line += "（本二进制未编译）";
    }
    let _ = writeln!(buf, "{cli_line}");
    let serve_line = format!(
        "  <app> {} [--addr :8080]      HTTP 模式（REST 路由 + /openapi.json + 可挂 /mcp）",
        serve
    );
    let serve_line = if caps.no_http {
        serve_line + "（已禁用）"
    } else if !crate::dispatch::http_frontend_compiled() {
        serve_line + "（本二进制未编译）"
    } else {
        serve_line
    };
    let _ = writeln!(buf, "{serve_line}");
    let mcp_line = format!(
        "  <app> {} stdio|http          MCP 模式（官方 SDK；--versions 限定协议版本）",
        mcp_word
    );
    let mcp_line = if caps.no_mcp {
        mcp_line + "（已禁用）"
    } else if !crate::dispatch::mcp_frontend_compiled() {
        mcp_line + "（本二进制未编译）"
    } else {
        mcp_line
    };
    let _ = writeln!(buf, "{mcp_line}");
    let _ = writeln!(
        buf,
        "内置参数（代码中的 xyz_rust::Config 或命令行）：--xyz.addr=:8080（默认监听地址） --xyz.bearer=tok1,tok2（serve 与 MCP http 的 Bearer 凭据）"
    );
    // CLI 被禁用时不生成子命令，总览也不再列出命令表。
    let names = reg.names();
    if names.is_empty() || caps.no_cli {
        write!(w, "{buf}")?;
        // 自定义 after 块在早退路径照打。
        return write_block(w, help_after);
    }
    let _ = writeln!(buf);
    let _ = writeln!(buf, "命令:");
    let width = names.iter().map(|n| n.len()).max().unwrap_or(0);
    for n in &names {
        let summary = reg.get(n).map(|e| e.summary.clone()).unwrap_or_default();
        let _ = writeln!(buf, "  {n:<width$}  {summary}");
    }
    write!(w, "{buf}")?;
    write_block(w, help_after)
}
