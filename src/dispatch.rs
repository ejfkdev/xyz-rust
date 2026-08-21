// 根派发器：读取进程参数，自行判断运行模式，按派发产出的退出码退出。
// 整个程序可以是一条 define 链。
//
// main（以及 main_config）派发进程级默认注册表并内部调用
// std::process::exit，因此 main 里写的清理代码不会在它们之后执行。需要
// 清理、自定义退出码或嵌入派发器时，用显式注册表的 run/run_config 版本，
// 它们只返回退出码。
//
// 模式探测：
//	<app> [命令] ...          -> CLI 前端（子命令、flag、位置参数、-h / -v）
//	<app> mcp stdio|http      -> MCP 前端（官方 SDK；--versions 钉定协议版本）
//	<app> serve [--addr ...]  -> HTTP 前端（REST + /openapi.json + /mcp）
//	<app>（无参数）| help     -> 总览
//
// 模式关键词默认为 serve/mcp/help 且是保留的顶层名字；两者都跟随
// run_config 里的 Modes 配置，可重命名。

use crate::config::Config;
use crate::ctx::Ctx;
use crate::errors;
use crate::registry::Registry;

/// 前端编译标记（总览标注用；对齐 Go 的 cliFrontend/httpFrontend 常量）。
pub const fn cli_frontend_compiled() -> bool {
    cfg!(feature = "cli")
}

pub const fn http_frontend_compiled() -> bool {
    cfg!(feature = "http")
}

pub const fn mcp_frontend_compiled() -> bool {
    cfg!(feature = "mcp")
}

/// main 注册传入的全部已构建命令定义（来自 define）、按进程参数派发
/// 默认注册表并按退出码退出。零个参数表示「命令已注册，只派发」。
/// 需要自取退出码（嵌入、测试、清理）或用显式注册表时，用 run/run_config。
pub fn main(cmds: &[&dyn crate::builder::Definable]) -> ! {
    if !cmds.is_empty() {
        for cmd in cmds {
            if let Err(e) = cmd.register(Registry::default()) {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(Registry::default(), args));
}

/// 带自定义配置的 main（如重命名模式词）。
pub fn main_config(cfg: Config) -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run_config(Registry::default(), args, cfg));
}

/// main 的显式参数与默认配置形态：返回退出码而不退出进程。
pub fn run(reg: &Registry, args: Vec<String>) -> i32 {
    run_config(reg, args, Config::default())
}

/// 带自定义配置的 run（重命名模式词、通道能力）。
pub fn run_config(reg: &Registry, args: Vec<String>, cfg: Config) -> i32 {
    let (serve, mcp_word, help_word) = match resolve_modes(&cfg) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if let Err(e) = check_reserved(reg, &serve, &mcp_word, &help_word) {
        eprintln!("{e}");
        return 2;
    }
    // 没有任何已注册命令：什么都不做，静默退出 0。
    if reg.names().is_empty() {
        return 0;
    }
    // 壳能力：-v/--version 由根派发器管，任何能力组合下都可用。
    for a in &args {
        if a == "-v" || a == "--version" {
            let bin = crate::cli::app::bin_name();
            println!("{bin} version {}", crate::version::version());
            return 0;
        }
    }
    // 内置参数 --xyz.*：剥离开分发给各前端（帮助/版本不受影响）。
    let mut cfg = cfg;
    let args = match crate::builtins::strip_xyz_flags(args, &mut cfg) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xyz: {e}");
            return 2;
        }
    };
    if cfg.log_level != crate::logx::Level::Unset {
        crate::logx::set_level(cfg.log_level);
    }
    if args.is_empty() || args[0] == help_word || args[0] == "--help" || args[0] == "-h" {
        let mut stdout = std::io::stdout();
        let _ =
            crate::overview::print_overview(&mut stdout, reg, &serve, &mcp_word, cfg.capabilities);
        return 0;
    }
    // 优雅关停：信号取消的 ctx 贯穿 CLI/HTTP/MCP，长任务可在退出前排空。
    let ctx = Ctx::new();
    spawn_signal_watcher(ctx.clone());
    crate::logx::debugf(format_args!(
        "dispatch: mode word='{}' addr={} tokens={} timeout={:?} cors={}",
        args[0],
        cfg.addr,
        cfg.bearer_tokens.len(),
        cfg.timeout,
        cfg.cors_origins.len()
    ));
    if args[0] == serve {
        if cfg.capabilities.no_http {
            crate::logx::warnf(format_args!(
                "{serve} 模式已被禁用（Config.Capabilities.NoHTTP）"
            ));
            return 1;
        }
        return run_serve(&ctx, reg, &args[1..], cfg);
    }
    if args[0] == mcp_word {
        if cfg.capabilities.no_mcp {
            crate::logx::warnf(format_args!(
                "{mcp_word} 模式已被禁用（Config.Capabilities.NoMCP）"
            ));
            return 1;
        }
        return run_mcp(&ctx, reg, &args[1..], cfg);
    }
    if cfg.capabilities.no_cli {
        crate::logx::warnf(format_args!(
            "子命令不可用：CLI 已禁用（Config.Capabilities.NoCLI；{mcp_word}/{serve}/help/-v 仍可用）"
        ));
        return 1;
    }
    run_cli(&ctx, reg, &args)
}

/// resolveModes 默认并校验模式词：必须是无前导横线的普通词且两两不同。
fn resolve_modes(cfg: &Config) -> errors::Result<(String, String, String)> {
    let mut serve = cfg.modes.serve.clone();
    let mut mcp_word = cfg.modes.mcp.clone();
    let mut help_word = cfg.modes.help.clone();
    if serve.is_empty() {
        serve = "serve".to_string();
    }
    if mcp_word.is_empty() {
        mcp_word = "mcp".to_string();
    }
    if help_word.is_empty() {
        help_word = "help".to_string();
    }
    for w in [&serve, &mcp_word, &help_word] {
        if w.starts_with('-') || w.chars().any(|c| c == ' ' || c == '\t') {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!("xyz: invalid mode word {w:?} (no leading dash, no whitespace)"),
            ));
        }
    }
    if serve == mcp_word || serve == help_word || mcp_word == help_word {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!(
                "xyz: mode words must be pairwise distinct (serve={serve:?} mcp={mcp_word:?} help={help_word:?})"
            ),
        ));
    }
    Ok((serve, mcp_word, help_word))
}

/// checkReserved 拒绝顶层段与模式词相撞的注册名（那些词归派发器）。
fn check_reserved(
    reg: &Registry,
    serve: &str,
    mcp_word: &str,
    help_word: &str,
) -> errors::Result<()> {
    for name in reg.names() {
        let top = name.split('.').next().unwrap_or("");
        if top == serve || top == mcp_word || top == help_word {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!(
                    "xyz: command {name:?}: top-level name {top:?} is reserved for mode dispatch"
                ),
            ));
        }
    }
    Ok(())
}

/// 信号接线：可用 tokio 时走 tokio::signal；纯 CLI 构建走 ctrlc。
fn spawn_signal_watcher(ctx: Ctx) {
    #[cfg(feature = "http-stack")]
    {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async move {
                let _ = tokio::signal::ctrl_c().await;
                ctx.cancel();
            });
        });
    }
    #[cfg(all(not(feature = "http-stack"), feature = "cli"))]
    {
        let _ = ctrlc::set_handler(move || {
            ctx.cancel();
        });
    }
}

// ---- 通道运行时路径（feature 裁剪；stub 与 Go 的 build-tag 口袋对齐）----

#[cfg(feature = "cli")]
fn run_cli(ctx: &Ctx, reg: &Registry, args: &[String]) -> i32 {
    crate::cli::run_context(ctx, reg, args, crate::cli::Options::default())
}

#[cfg(not(feature = "cli"))]
fn run_cli(_ctx: &Ctx, _reg: &Registry, _args: &[String]) -> i32 {
    eprintln!("xyz: 本二进制未编译 CLI 前端（构建时使用了 --no-default-features）");
    1
}

#[cfg(feature = "http")]
fn run_serve(ctx: &Ctx, reg: &Registry, args: &[String], cfg: Config) -> i32 {
    crate::httpapi::serve(ctx, reg, args, cfg)
}

#[cfg(not(feature = "http"))]
fn run_serve(_ctx: &Ctx, _reg: &Registry, _args: &[String], _cfg: Config) -> i32 {
    eprintln!("xyz: 本二进制未编译 HTTP 前端（构建时禁用了 http feature）");
    1
}

#[cfg(feature = "mcp")]
fn run_mcp(ctx: &Ctx, reg: &Registry, args: &[String], cfg: Config) -> i32 {
    crate::mcp::run_with_config(ctx, reg, args, cfg)
}

#[cfg(not(feature = "mcp"))]
fn run_mcp(_ctx: &Ctx, _reg: &Registry, _args: &[String], _cfg: Config) -> i32 {
    eprintln!("xyz: 本二进制未编译 MCP 前端（构建时禁用了 mcp feature）");
    1
}
