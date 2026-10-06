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
    run_internal(reg, args, cfg, false).0
}

/// 可组合派发：与 run_config 同管线，但当参数进入 CLI 模式且首段不是任何
/// 已注册命令段/别名（也不是 flag）时，不打印任何东西、返回 (0, false)，
/// 由宿主路由其余参数（false 即「未命中」）。其余路径行为与 run_config
/// 完全一致。
pub fn try_run(reg: &Registry, args: Vec<String>) -> (i32, bool) {
    try_run_config(reg, args, Config::default())
}

/// 带自定义配置的 try_run。
pub fn try_run_config(reg: &Registry, args: Vec<String>, cfg: Config) -> (i32, bool) {
    run_internal(reg, args, cfg, true)
}

/// CLI 首段是否命中树（已知命令段、别名或 default 子命令）。
fn cli_known_top(reg: &Registry, first: &str) -> bool {
    if first.starts_with('-') {
        return true; // flag/帮助等交给 CLI 自身路径
    }
    for e in reg.all() {
        if e.cli.skip {
            continue;
        }
        if e.name.split('.').next() == Some(first) {
            return true;
        }
        if e.cli.aliases.iter().any(|a| a == first) {
            return true;
        }
        if e.cli.default {
            return true; // 默认子命令吞掉未知名
        }
    }
    false
}

fn run_internal(reg: &Registry, args: Vec<String>, cfg: Config, composable: bool) -> (i32, bool) {
    let words = match resolve_modes(&cfg) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return (2, true);
        }
    };
    // 没有任何已注册命令：什么都不做，静默退出 0。
    if reg.names().is_empty() {
        return (0, true);
    }
    // 壳能力：-v/--version 由根派发器管，任何能力组合下都可用
    // （"--" 之后的 token 一律是位置参数，不再识别 -v）。
    for a in &args {
        if a == "--" {
            break;
        }
        if a == "-v" || a == "--version" {
            let bin = crate::cli::app::bin_name();
            println!("{bin} version {}", crate::version::version());
            return (0, true);
        }
    }
    // 内置参数 --xyz.*：剥离开分发给各前端（帮助/版本不受影响）。
    let mut cfg = cfg;
    let args = match crate::builtins::strip_xyz_flags(args, &mut cfg) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xyz: {e}");
            return (2, true);
        }
    };
    if cfg.log_level != crate::logx::Level::Unset {
        crate::logx::set_level(cfg.log_level);
    }
    // 界面语言：--xyz.lang（已写回 cfg）> Config.lang > 环境检测 > 英文。
    let resolved_lang = if !cfg.lang.is_empty() {
        match crate::lang::XyzLang::parse(&cfg.lang) {
            Some(l) => l,
            None => {
                eprintln!("xyz: invalid --xyz.lang {:?} (want en|zh-CN)", cfg.lang);
                return (2, true);
            }
        }
    } else {
        crate::lang::XyzLang::detect()
    };
    crate::lang::set(
        resolved_lang,
        cfg.translations.get(resolved_lang.as_str()).cloned(),
    );
    // 遮蔽让位（spec §13.1）：顶层段等于模式词的用户命令让裸词改为路由到
    // 用户命令；内建模式经 xyz.<词> 仍可达。
    let shadowed = shadowed_modes(reg, &words);
    // 总览触发（§13.2 第 4 步）：空参数 / 根 -h/--help。裸 help 词交给
    // 模式匹配（被遮蔽时归用户命令）。
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        let mut stdout = std::io::stdout();
        let _ = crate::overview::print_overview(
            &mut stdout,
            reg,
            &words,
            &shadowed,
            cfg.capabilities,
            &cfg.help_before,
            &cfg.help_after,
        );
        return (0, true);
    }
    // 模式派发：xyz.<词> 恒命中；裸词仅未被遮蔽时命中（§13.2 第 5 步）。
    let kind = match_mode(&args[0], &words, &shadowed);
    if kind != ModeKind::None {
        let rest = &args[1..];
        if kind == ModeKind::Help {
            return (run_help(reg, rest, &words, &shadowed, cfg), true);
        }
        // serve/http/mcp 的 -h/--help：打印模式帮助而非起服务。
        if has_help_flag(rest) {
            return (print_mode_help(kind, &words), true);
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
        match kind {
            ModeKind::Serve => {
                if cfg.capabilities.no_http {
                    crate::logx::warnf(format_args!(
                        "{}",
                        crate::lang::tf("warn.mode_disabled", &[&words.serve, "HTTP"])
                    ));
                    return (1, true);
                }
                // serve：REST + /openapi.json + 挂 /mcp（is_mcp 依 no_mcp 裁剪）。
                let mount = !cfg.capabilities.no_mcp;
                return (run_serve(&ctx, reg, rest, cfg, mount), true);
            }
            ModeKind::Http => {
                if cfg.capabilities.no_http {
                    crate::logx::warnf(format_args!(
                        "{}",
                        crate::lang::tf("warn.mode_disabled", &[&words.http, "HTTP"])
                    ));
                    return (1, true);
                }
                // http：仅 REST + /openapi.json，不挂 /mcp（spec §13.1）。
                return (run_serve(&ctx, reg, rest, cfg, false), true);
            }
            _ => {}
        }
        let mcp_word = words.mcp.clone();
        if cfg.capabilities.no_mcp {
            crate::logx::warnf(format_args!(
                "{}",
                crate::lang::tf("warn.mode_disabled", &[&mcp_word, "MCP"])
            ));
            return (1, true);
        }
        return (run_mcp(&ctx, reg, rest, cfg), true);
    }
    if cfg.capabilities.no_cli {
        crate::logx::warnf(format_args!(
            "{}",
            crate::lang::tf("warn.no_cli", &[&words.mcp, &words.serve])
        ));
        return (1, true);
    }
    if composable && !args.is_empty() && !cli_known_top(reg, &args[0]) {
        // 宿主兜底：静默交还，不做任何输出。
        return (0, false);
    }
    let ctx = Ctx::new();
    spawn_signal_watcher(ctx.clone());
    (run_cli(&ctx, reg, &args, &cfg), true)
}

/// 解析后的四个模式词（spec §13.1）。
pub struct Words {
    pub(crate) serve: String,
    pub(crate) http: String,
    pub(crate) mcp: String,
    pub(crate) help: String,
}

impl Words {
    // （pub(crate)：overview 读取词面）
    fn all(&self) -> [&str; 4] {
        [
            self.serve.as_str(),
            self.http.as_str(),
            self.mcp.as_str(),
            self.help.as_str(),
        ]
    }
}

/// 命中的内建模式（spec §13.2 第 5 步）。
#[derive(PartialEq, Eq, Clone, Copy)]
enum ModeKind {
    Serve,
    Http,
    Mcp,
    Help,
    None,
}

/// resolveModes 默认并校验模式词：必须是无前导横线的普通词且两两不同
/// （四词：serve/http/mcp/help，spec §13.1）。
fn resolve_modes(cfg: &Config) -> errors::Result<Words> {
    let pick = |v: &str, dflt: &str| {
        if v.is_empty() {
            dflt.to_string()
        } else {
            v.to_string()
        }
    };
    let w = Words {
        serve: pick(&cfg.modes.serve, "serve"),
        http: pick(&cfg.modes.http, "http"),
        mcp: pick(&cfg.modes.mcp, "mcp"),
        help: pick(&cfg.modes.help, "help"),
    };
    for word in w.all() {
        if word.starts_with('-') || word.chars().any(|c| c == ' ' || c == '\t') {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!("xyz: invalid mode word {word:?} (no leading dash, no whitespace)"),
            ));
        }
    }
    let words = w.all();
    for i in 0..words.len() {
        for j in (i + 1)..words.len() {
            if words[i] == words[j] {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!(
                        "xyz: mode words must be pairwise distinct (serve={:?} http={:?} mcp={:?} help={:?})",
                        w.serve, w.http, w.mcp, w.help
                    ),
                ));
            }
        }
    }
    Ok(w)
}

/// 计算哪些模式词被用户命令的顶层段遮蔽（spec §13.1）：遮蔽时裸词让位给
/// 用户命令，内建模式仅经 `xyz.<词>` 可达；CLI-Skip 的命令不参与遮蔽。
fn shadowed_modes(reg: &Registry, w: &Words) -> std::collections::BTreeSet<String> {
    let mut tops = std::collections::BTreeSet::new();
    for name in reg.names() {
        if let Some(e) = reg.get(&name)
            && e.cli.skip
        {
            continue;
        }
        if let Some(top) = name.split('.').next() {
            tops.insert(top.to_string());
        }
    }
    w.all()
        .iter()
        .filter(|word| tops.contains(**word))
        .map(|word| word.to_string())
        .collect()
}

/// 首 token → 内建模式：`xyz.<词>` 恒命中；裸词仅在未被遮蔽时命中。
fn match_mode(token: &str, w: &Words, shadowed: &std::collections::BTreeSet<String>) -> ModeKind {
    if let Some(rest) = token.strip_prefix("xyz.") {
        return match rest {
            r if r == w.serve => ModeKind::Serve,
            r if r == w.http => ModeKind::Http,
            r if r == w.mcp => ModeKind::Mcp,
            r if r == w.help => ModeKind::Help,
            _ => ModeKind::None,
        };
    }
    if shadowed.contains(token) {
        return ModeKind::None;
    }
    match token {
        t if t == w.serve => ModeKind::Serve,
        t if t == w.http => ModeKind::Http,
        t if t == w.mcp => ModeKind::Mcp,
        t if t == w.help => ModeKind::Help,
        _ => ModeKind::None,
    }
}

/// args 中（"--" 之前）是否出现 -h/--help。
fn has_help_flag(args: &[String]) -> bool {
    for a in args {
        if a == "--" {
            break;
        }
        if a == "-h" || a == "--help" {
            return true;
        }
    }
    false
}

/// `help` 模式（spec §10.4/§13.2）：裸 help → 总览；help <模式> → 模式帮助；
/// help <命令路径> → 该命令详细帮助（点分或空格分隔皆可，等同 `<path> -h`）。
fn run_help(
    reg: &Registry,
    rest: &[String],
    w: &Words,
    shadowed: &std::collections::BTreeSet<String>,
    cfg: Config,
) -> i32 {
    if rest.is_empty() {
        let mut stdout = std::io::stdout();
        let _ = crate::overview::print_overview(
            &mut stdout,
            reg,
            w,
            shadowed,
            cfg.capabilities,
            &cfg.help_before,
            &cfg.help_after,
        );
        return 0;
    }
    let kind = match_mode(&rest[0], w, shadowed);
    match kind {
        ModeKind::Help | ModeKind::None if kind == ModeKind::Help => {
            let mut stdout = std::io::stdout();
            let _ = crate::overview::print_overview(
                &mut stdout,
                reg,
                w,
                shadowed,
                cfg.capabilities,
                &cfg.help_before,
                &cfg.help_after,
            );
            0
        }
        ModeKind::Serve | ModeKind::Http | ModeKind::Mcp => print_mode_help(kind, w),
        _ => {
            // 命令路径：token 内的点再拆（help user.add == help user add）。
            let mut path: Vec<String> = Vec::new();
            for r in rest {
                path.extend(r.split('.').map(|seg| seg.to_string()));
            }
            if cfg.capabilities.no_cli {
                crate::logx::warnf(format_args!(
                    "{}",
                    crate::lang::tf("warn.no_cli", &[&w.mcp, &w.serve])
                ));
                return 1;
            }
            path.push("-h".to_string());
            run_cli(&Ctx::new(), reg, &path, &cfg)
        }
    }
}

/// 打印某个模式的帮助（spec §10.4/§13.2），不起服务。
fn print_mode_help(kind: ModeKind, w: &Words) -> i32 {
    let key = match kind {
        ModeKind::Serve => "mode_help.serve",
        ModeKind::Http => "mode_help.http",
        ModeKind::Mcp => "mode_help.mcp",
        _ => "mode_help.help",
    };
    let word = match kind {
        ModeKind::Serve => &w.serve,
        ModeKind::Http => &w.http,
        ModeKind::Mcp => &w.mcp,
        _ => &w.help,
    };
    println!("{}", crate::lang::tf(key, &[word]));
    0
}

/// 信号接线：可用 tokio 时走 tokio::signal；纯 CLI 构建走 ctrlc。
fn spawn_signal_watcher(ctx: Ctx) {
    // 第三态（http-stack 与 cli 都裁掉）：函数体为空，引用参数抑警告。
    #[cfg(not(any(feature = "http-stack", feature = "cli")))]
    let _ = &ctx;

    #[cfg(feature = "http-stack")]
    {
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
fn run_cli(ctx: &Ctx, reg: &Registry, args: &[String], cfg: &Config) -> i32 {
    // --xyz.format 的默认格式经 Options 注入 CLI 前端（spec §10.7）。
    crate::cli::run_context(
        ctx,
        reg,
        args,
        crate::cli::Options {
            format: Some(cfg.format.clone()),
            format_from_flag: cfg.format_from_flag,
            format_interactive: Some(cfg.format_interactive.clone()),
            format_piped: Some(cfg.format_piped.clone()),
            ..Default::default()
        },
    )
}

#[cfg(not(feature = "cli"))]
fn run_cli(_ctx: &Ctx, _reg: &Registry, _args: &[String], _cfg: &Config) -> i32 {
    eprintln!("xyz: {}", crate::lang::tf("stub.not_compiled", &["CLI"]));
    1
}

#[cfg(feature = "http")]
fn run_serve(ctx: &Ctx, reg: &Registry, args: &[String], cfg: Config, mount_mcp: bool) -> i32 {
    crate::httpapi::serve(ctx, reg, args, cfg, mount_mcp)
}

#[cfg(not(feature = "http"))]
fn run_serve(_ctx: &Ctx, _reg: &Registry, _args: &[String], _cfg: Config, _mount_mcp: bool) -> i32 {
    eprintln!("xyz: {}", crate::lang::tf("stub.not_compiled", &["HTTP"]));
    1
}

#[cfg(feature = "mcp")]
fn run_mcp(ctx: &Ctx, reg: &Registry, args: &[String], cfg: Config) -> i32 {
    crate::mcp::run_with_config(ctx, reg, args, cfg)
}

#[cfg(not(feature = "mcp"))]
fn run_mcp(_ctx: &Ctx, _reg: &Registry, _args: &[String], _cfg: Config) -> i32 {
    eprintln!("xyz: {}", crate::lang::tf("stub.not_compiled", &["MCP"]));
    1
}
