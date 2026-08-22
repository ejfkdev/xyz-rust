// lang 是内置界面文本的 i18n 层：语言枚举 + 进程级目录 + 用户覆盖。默认
// 英文（xyz 的规范默认）；中文随库携带。选择顺序（根派发器负责落地）：
// --xyz.lang flag > Config.lang > LANG/LC_ALL 环境检测 > 英文。
//
// t(key) 返回当前语言文本；tf(key, params) 用 {0}、{1}… 占位符代入。
// 覆盖表（Config.translations）优先于内置译文；未命中回退键名本身（绝不
// panic）。键名与英文文案与 xyz-go 的 langx 目录逐键一致（xyz-spec
// §15.8 的规范键表），传输相关键（mcp 模式行/用法）按本 SDK 能力取值。

use std::collections::HashMap;
use std::sync::RwLock;

/// 受支持的语言。默认英文（规范默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum XyzLang {
    #[default]
    En,
    ZhCn,
}

impl XyzLang {
    /// --xyz.lang 的取值形态。
    pub fn as_str(&self) -> &'static str {
        match self {
            XyzLang::En => "en",
            XyzLang::ZhCn => "zh-CN",
        }
    }

    /// 解析 --xyz.lang 值；未知值 None（注册期报错）。
    pub fn parse(s: &str) -> Option<XyzLang> {
        match s {
            "en" => Some(XyzLang::En),
            "zh-CN" => Some(XyzLang::ZhCn),
            _ => None,
        }
    }

    /// LANG/LC_ALL 环境检测：zh 前缀 → 中文；其余 → 英文。
    pub fn detect() -> XyzLang {
        for key in ["LC_ALL", "LANG"] {
            if let Ok(v) = std::env::var(key) {
                if v.is_empty() {
                    continue;
                }
                let lower = v.to_ascii_lowercase();
                if lower.starts_with("zh") {
                    return XyzLang::ZhCn;
                }
                return XyzLang::En;
            }
        }
        XyzLang::En
    }
}

static STATE: RwLock<(XyzLang, Option<HashMap<String, String>>)> = RwLock::new((XyzLang::En, None));

/// 设置进程级语言与可选覆盖表（None = 无覆盖）。嵌入场景可自行调用。
pub fn set(lang: XyzLang, overrides: Option<HashMap<String, String>>) {
    *STATE.write().unwrap() = (lang, overrides);
}

/// 当前语言（零配置下 En）。
pub fn lang() -> XyzLang {
    STATE.read().unwrap().0
}

/// 消息目录：en 为规范文案（xyz-spec §15.8 键表），zh-CN 为随库译文。
const EN: &[(&str, &str)] = &[
    (
        "overview.usage_line",
        "Usage (the mode is detected automatically; definitions live in one place):",
    ),
    (
        "overview.cli_mode",
        "  <app> [command] [args]         CLI mode (subcommands + flags/positionals; -h help, -v version)",
    ),
    (
        "overview.serve_mode",
        "  <app> {0} [--addr :8080]    HTTP mode (REST routes + /openapi.json + optional /mcp)",
    ),
    (
        "overview.mcp_mode",
        "  <app> {0} stdio|http        MCP mode (official SDK; --versions pins revisions)",
    ),
    (
        "overview.builtins",
        "Built-in parameters (xyz.Config in code or on the command line): --xyz.addr=:8080 (default listen address) --xyz.bearer=tok1,tok2 (Bearer credentials for serve and MCP http)",
    ),
    ("overview.commands", "Commands:"),
    ("overview.disabled", " (disabled)"),
    ("overview.not_compiled", " (not compiled into this binary)"),
    ("help.usage", "Usage:"),
    ("help.aliases", "Aliases:"),
    ("help.commands", "Commands:"),
    ("help.flags", "Flags:"),
    ("help.global_flags", "Global Flags:"),
    ("help.commands_placeholder", "[command]"),
    ("help.flags_placeholder", "[flags]"),
    (
        "help.json_flag",
        "output JSON instead of the human-readable form",
    ),
    ("help.version_flag", "print version information"),
    ("help.help_flag", "print help"),
    (
        "cli.err_positional_count",
        "{0}: positional argument count mismatch (want {1} to {2}, got {3})",
    ),
    (
        "warn.mode_disabled",
        "{0} mode was disabled (Config.Capabilities.No{1})",
    ),
    (
        "warn.no_cli",
        "subcommands unavailable: CLI is disabled (Config.Capabilities.NoCLI; {0}/{1}/help/-v remain available)",
    ),
    (
        "warn.bearer_stdio",
        "Bearer credential checks only apply to the http transport; stdio is a local process and is not protected",
    ),
    (
        "stub.not_compiled",
        "this binary was built without the {0} frontend",
    ),
    (
        "log.serve_listening",
        "listening on {0}://{1} (REST + /openapi.json{2})",
    ),
    ("log.graceful", "gracefully shut down (ctx cancelled)"),
    ("log.mcp_listening", "MCP listening on {0}"),
    ("log.cors_on", "CORS enabled: {0}"),
    (
        "log.debug_dispatch",
        "dispatch: mode word='{0}' addr={1} tokens={2} timeout={3:?} cors={4}",
    ),
    (
        "mcp.usage",
        "usage: mcp stdio|http [--addr :8080] [--versions 2025-06-18,2026-07-28] [--name N] [--server-version V]",
    ),
    ("mcp.err_missing_transport", "missing transport"),
    (
        "mcp.err_unknown_transport",
        "unknown transport {0} (want stdio|http)",
    ),
    (
        "mcp.err_sse_removed",
        "this SDK removed the legacy HTTP+SSE transport with the 2026-07-28 revision (available: stdio|http)",
    ),
    (
        "mcp.err_unknown_version",
        "unknown protocol version {0} (known: {1})",
    ),
    (
        "mcp.err_empty_version",
        "empty protocol version in --versions",
    ),
    (
        "mcp.err_transport_versions",
        "transport {0} cannot serve any of the requested versions {1}",
    ),
    ("mcp.err_usage_extra_arg", "unexpected argument {0}"),
];

const ZH: &[(&str, &str)] = &[
    (
        "overview.usage_line",
        "用法（模式由程序自动判断，定义只有一份）:",
    ),
    (
        "overview.cli_mode",
        "  <app> [命令] [参数]           CLI 模式（子命令 + flag/位置参数；-h 帮助，-v 版本）",
    ),
    (
        "overview.serve_mode",
        "  <app> {0} [--addr :8080]      HTTP 模式（REST 路由 + /openapi.json + 可挂 /mcp）",
    ),
    (
        "overview.mcp_mode",
        "  <app> {0} stdio|http          MCP 模式（官方 SDK；--versions 限定协议版本）",
    ),
    (
        "overview.builtins",
        "内置参数（代码中的 xyz_rust::Config 或命令行）：--xyz.addr=:8080（默认监听地址） --xyz.bearer=tok1,tok2（serve 与 MCP http 的 Bearer 凭据）",
    ),
    ("overview.commands", "命令:"),
    ("overview.disabled", "（已禁用）"),
    ("overview.not_compiled", "（本二进制未编译）"),
    ("help.usage", "Usage:"),
    ("help.aliases", "Aliases:"),
    ("help.commands", "命令:"),
    ("help.flags", "Flags:"),
    ("help.global_flags", "Global Flags:"),
    ("help.commands_placeholder", "[命令]"),
    ("help.flags_placeholder", "[flags]"),
    ("help.json_flag", "输出 JSON 而不是人类可读格式"),
    ("help.version_flag", "输出版本信息"),
    ("help.help_flag", "打印帮助"),
    (
        "cli.err_positional_count",
        "{0}: 位置参数数量不符（需要 {1} 到 {2} 个，收到 {3} 个）",
    ),
    (
        "warn.mode_disabled",
        "{0} 模式已被禁用（Config.Capabilities.No{1}）",
    ),
    (
        "warn.no_cli",
        "子命令不可用：CLI 已禁用（Config.Capabilities.NoCLI；{0}/{1}/help/-v 仍可用）",
    ),
    (
        "warn.bearer_stdio",
        "Bearer 凭据校验只作用于 http 传输，stdio 为本地进程不受保护",
    ),
    ("stub.not_compiled", "本二进制未编译 {0} 前端"),
    (
        "log.serve_listening",
        "监听 {0}://{1}（REST + /openapi.json{2}）",
    ),
    ("log.graceful", "已优雅关停（ctx 取消）"),
    ("log.mcp_listening", "MCP 监听 {0}"),
    ("log.cors_on", "CORS 开启：{0}"),
    (
        "log.debug_dispatch",
        "dispatch: mode word={0} addr={1} tokens={2} timeout={3} cors={4}",
    ),
    (
        "mcp.usage",
        "用法: mcp stdio|http [--addr :8080] [--versions 2025-06-18,2026-07-28]",
    ),
    ("mcp.err_missing_transport", "missing transport"),
    (
        "mcp.err_unknown_transport",
        "unknown transport {0} (want stdio|http)",
    ),
    (
        "mcp.err_sse_removed",
        "本 SDK（官方 Rust SDK rmcp）已随 2026-07-28 修订移除 HTTP+SSE 传输（可用 stdio|http）",
    ),
    (
        "mcp.err_unknown_version",
        "unknown protocol version {0} (known: {1})",
    ),
    (
        "mcp.err_empty_version",
        "empty protocol version in --versions",
    ),
    (
        "mcp.err_transport_versions",
        "transport {0} cannot serve any of the requested versions {1}",
    ),
    ("mcp.err_usage_extra_arg", "unexpected argument {0}"),
];

fn lookup(lang: XyzLang, key: &str) -> String {
    let table = match lang {
        XyzLang::ZhCn => ZH,
        XyzLang::En => EN,
    };
    table
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_else(|| key.to_string())
}

/// 当前语言下 key 的文本：覆盖 > 内置 > 键名回退。
pub fn t(key: &str) -> String {
    let guard = STATE.read().unwrap();
    let lang = guard.0;
    let overridden = guard.1.as_ref().and_then(|ov| ov.get(key).cloned());
    drop(guard);
    if let Some(s) = overridden {
        return s;
    }
    lookup(lang, key)
}

/// 带参数的 t：模板中的 {0}、{1}… 依次替换。
pub fn tf(key: &str, params: &[&str]) -> String {
    let mut out = t(key);
    for (i, p) in params.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_default() {
        assert_eq!(XyzLang::parse("en"), Some(XyzLang::En));
        assert_eq!(XyzLang::parse("zh-CN"), Some(XyzLang::ZhCn));
        assert_eq!(XyzLang::parse("fr"), None);
        assert_eq!(XyzLang::default(), XyzLang::En);
    }

    #[test]
    fn catalog_and_overrides() {
        set(XyzLang::En, None);
        assert!(t("overview.usage_line").starts_with("Usage ("));
        set(XyzLang::ZhCn, None);
        assert!(t("overview.usage_line").starts_with("用法（"));
        // 覆盖优先、未知键回退、tf 占位
        set(
            XyzLang::En,
            Some(HashMap::from([(
                "help.help_flag".to_string(),
                "show me help".to_string(),
            )])),
        );
        assert_eq!(t("help.help_flag"), "show me help");
        assert_eq!(t("no.such.key"), "no.such.key");
        assert_eq!(
            tf("warn.mode_disabled", &["serve", "HTTP"]),
            "serve mode was disabled (Config.Capabilities.NoHTTP)"
        );
        set(XyzLang::En, None);
    }
}
