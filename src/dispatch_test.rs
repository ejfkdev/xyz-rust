// 根派发器测试（Go main_test.go 对应物）：总览、空注册表 no-op、模式词、
// 保留字、版本 flag、能力开关、--xyz.* 剥离。

use crate::Ctx;
use crate::config::{Capabilities, Config, ModeWords};
use crate::dispatch::{run, run_config};
use crate::errors;
use crate::registry::Registry;
use crate::spec::command::Command;
use xyz_rust::XyzArgs;

#[derive(XyzArgs)]
struct TArgs {
    #[xyz(desc = "s")]
    s: String,
}

fn th(_: &Ctx, in_: &TArgs) -> errors::Result<String> {
    Ok(in_.s.clone())
}

fn test_reg(names: &[&str]) -> Registry {
    let reg = Registry::new();
    for n in names {
        Command::new(n, th).register(&reg).unwrap();
    }
    reg
}

fn test_words() -> crate::dispatch::Words {
    crate::dispatch::Words {
        serve: "serve".to_string(),
        http: "http".to_string(),
        mcp: "mcp".to_string(),
        help: "help".to_string(),
    }
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn run_overview_forms() {
    let reg = test_reg(&["a.b"]);
    assert_eq!(run(&reg, vec![]), 0);
    assert_eq!(run(&reg, args(&["help"])), 0);
    assert_eq!(run(&reg, args(&["--help"])), 0);
    assert_eq!(run(&reg, args(&["-h"])), 0);
}

#[test]
fn run_empty_registry_is_noop() {
    for argv in [
        vec![],
        args(&["help"]),
        args(&["user", "add"]),
        args(&["mcp", "stdio"]),
        args(&["serve"]),
    ] {
        assert_eq!(run(&Registry::new(), argv), 0);
    }
}

#[test]
fn run_custom_mode_words() {
    let cfg = Config {
        modes: ModeWords {
            serve: "httpd".into(),
            http: String::new(),
            mcp: "protocol".into(),
            help: "assist".into(),
        },
        ..Default::default()
    };
    let reg = test_reg(&["a.b"]);
    // 自定义帮助词走总览
    assert_eq!(run_config(&reg, args(&["assist"]), cfg.clone()), 0);
    // httpd 词仍归 HTTP 模式；真实监听会阻塞，用 NoHTTP 挡在半路验证分发
    let mut no_http = cfg;
    no_http.capabilities.no_http = true;
    assert_eq!(run_config(&reg, args(&["httpd"]), no_http), 1);
}

#[test]
fn run_invalid_mode_words() {
    let reg = test_reg(&["a.b"]);
    for cfg in [
        Config {
            modes: ModeWords {
                serve: "serve".into(),
                http: String::new(),
                mcp: "serve".into(),
                help: String::new(),
            },
            ..Default::default()
        },
        Config {
            modes: ModeWords {
                serve: "-serve".into(),
                http: String::new(),
                mcp: String::new(),
                help: String::new(),
            },
            ..Default::default()
        },
        Config {
            modes: ModeWords {
                serve: "sv c".into(),
                http: String::new(),
                mcp: String::new(),
                help: String::new(),
            },
            ..Default::default()
        },
    ] {
        assert_eq!(run_config(&reg, vec![], cfg), 2);
    }
}

#[test]
fn mode_words_are_no_longer_reserved() {
    // spec §13.1（v0.4.4）：旧的硬保留已废除——顶层段撞模式词只是遮蔽
    // （裸词让位给用户命令，xyz.<词> 仍达内建），注册不再报错。
    for name in ["serve.x", "mcp.up", "help.me", "http.get"] {
        let reg = Registry::new();
        Command::new(name, th).register(&reg).unwrap();
    }
}

#[test]
fn run_version_flag_anywhere() {
    let reg = test_reg(&["a.b"]);
    for argv in [
        args(&["-v"]),
        args(&["--version"]),
        args(&["echo", "hi", "-v"]),
    ] {
        assert_eq!(run(&reg, argv), 0);
    }
}

#[test]
fn run_disabled_capabilities() {
    let reg = test_reg(&["echo.hi"]);

    // NoCLI：子命令不可用，但 help/-v/serve/mcp 壳能力保留。
    let mut no_cli = Config::default();
    no_cli.capabilities.no_cli = true;
    assert_eq!(
        run_config(&reg, args(&["echo", "hi", "--s", "x"]), no_cli.clone()),
        1
    );
    assert_eq!(run_config(&reg, args(&["help"]), no_cli.clone()), 0);
    assert_eq!(run_config(&reg, args(&["-v"]), no_cli.clone()), 0);

    // NoMCP：mcp 模式被拒绝（配置检查在进入前端前，不阻塞）。
    let mut no_mcp = Config::default();
    no_mcp.capabilities.no_mcp = true;
    assert_eq!(run_config(&reg, args(&["mcp", "stdio"]), no_mcp), 1);

    // NoHTTP：serve 模式被拒绝。
    let no_http = Config {
        capabilities: Capabilities {
            no_http: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(run_config(&reg, args(&["serve"]), no_http), 1);

    // 叠加同样生效。
    let both = Config {
        capabilities: Capabilities {
            no_cli: true,
            no_http: true,
            no_mcp: false,
        },
        ..Default::default()
    };
    assert_eq!(run_config(&reg, args(&["serve"]), both), 1);
}

#[test]
fn strip_xyz_flags() {
    let mut cfg = Config {
        bearer_tokens: vec!["code-tok".into()],
        ..Default::default()
    };
    let rest = crate::builtins::strip_xyz_flags(
        args(&[
            "--xyz.bearer=a,b",
            "mcp",
            "stdio",
            "--xyz.addr=:9090",
            "--xyz.bearer=b",
        ]),
        &mut cfg,
    )
    .unwrap();
    assert_eq!(cfg.addr, ":9090");
    assert_eq!(cfg.bearer_tokens, vec!["code-tok", "a", "b"]);
    assert_eq!(rest, args(&["mcp", "stdio"]));

    // 分开写法与空值去重
    let mut cfg2 = Config::default();
    let rest2 =
        crate::builtins::strip_xyz_flags(args(&["--xyz.bearer", "x,,y", "echo"]), &mut cfg2)
            .unwrap();
    assert_eq!(cfg2.bearer_tokens, vec!["x", "y"]);
    assert_eq!(rest2, args(&["echo"]));

    // 日志级别 / 超时 / TLS / CORS
    let mut cfg3 = Config::default();
    let rest3 = crate::builtins::strip_xyz_flags(
        args(&[
            "serve",
            "--xyz.log-level=debug",
            "--xyz.timeout",
            "45s",
            "--xyz.tls-cert=a.pem",
            "--xyz.tls-key",
            "k.pem",
            "--xyz.cors=x,y,z",
        ]),
        &mut cfg3,
    )
    .unwrap();
    assert_eq!(cfg3.log_level, crate::logx::Level::Debug);
    assert_eq!(cfg3.timeout, std::time::Duration::from_secs(45));
    assert_eq!(cfg3.cert_file, "a.pem");
    assert_eq!(cfg3.key_file, "k.pem");
    assert_eq!(cfg3.cors_origins, vec!["x", "y", "z"]);
    assert_eq!(rest3, args(&["serve"]));

    // 非法值在解析期报错
    assert!(
        crate::builtins::strip_xyz_flags(
            args(&["--xyz.log-level=verbose"]),
            &mut Config::default()
        )
        .is_err()
    );
    assert!(
        crate::builtins::strip_xyz_flags(args(&["--xyz.timeout=nope"]), &mut Config::default())
            .is_err()
    );
}

#[test]
fn lang_resolution_and_catalog() {
    // 目录随语言切换；覆盖表生效
    crate::lang::set(crate::lang::XyzLang::En, None);
    let reg = test_reg(&["a.b"]);
    let mut buf = Vec::new();
    crate::overview::print_overview(
        &mut buf,
        &reg,
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        "",
        "",
    )
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("Usage (the mode is detected"), "{out}");
    crate::lang::set(crate::lang::XyzLang::ZhCn, None);
    let mut buf2 = Vec::new();
    crate::overview::print_overview(
        &mut buf2,
        &reg,
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        "",
        "",
    )
    .unwrap();
    let out2 = String::from_utf8(buf2).unwrap();
    assert!(out2.contains("用法（模式由程序自动判断"), "{out2}");
    // v0.4.4：四模式行（http/help 为新增）随语言目录输出。
    assert!(out.contains("no /mcp"), "{out}");
    assert!(
        out.contains("detailed help for a command or a mode"),
        "{out}"
    );
    assert!(out2.contains("不挂 /mcp"), "{out2}");
    assert!(out2.contains("某命令或某模式的详细帮助"), "{out2}");
    // 覆盖表
    let mut ov = std::collections::HashMap::new();
    ov.insert("overview.commands".to_string(), "Commands!:".to_string());
    crate::lang::set(crate::lang::XyzLang::En, Some(ov));
    let mut buf3 = Vec::new();
    crate::overview::print_overview(
        &mut buf3,
        &reg,
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        "",
        "",
    )
    .unwrap();
    assert!(String::from_utf8(buf3).unwrap().contains("Commands!:"),);
    crate::lang::set(crate::lang::XyzLang::En, None);

    // --xyz.lang 非法值在解析期报错
    let mut cfg = Config::default();
    assert!(crate::builtins::strip_xyz_flags(args(&["--xyz.lang=fr"]), &mut cfg).is_err());
    let rest =
        crate::builtins::strip_xyz_flags(args(&["--xyz.lang=zh-CN", "help"]), &mut cfg).unwrap();
    assert_eq!(cfg.lang, "zh-CN");
    assert_eq!(rest, args(&["help"]));
}

#[test]
fn overview_help_blocks() {
    // 通过 print_overview 直接断言块插入与归一化
    let reg = test_reg(&["a.b"]);
    let mut buf = Vec::new();
    let before = "myapp v1.2.3 — do the thing\nhttps://github.com/me/myapp";
    let after = "Need help? https://github.com/me/myapp#faq";
    crate::overview::print_overview(
        &mut buf,
        &reg,
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        before,
        after,
    )
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.starts_with(&format!("{before}\n")), "{out}");
    assert!(out.ends_with(&format!("{after}\n")), "{out}");
    // 空块零变化
    let mut a = Vec::new();
    let mut b = Vec::new();
    crate::overview::print_overview(
        &mut a,
        &reg,
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        "",
        "",
    )
    .unwrap();
    crate::overview::print_overview(
        &mut b,
        &reg,
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        "",
        "",
    )
    .unwrap();
    assert_eq!(a, b);
    // 空注册表早退路径 after 照打
    let mut c = Vec::new();
    crate::overview::print_overview(
        &mut c,
        &Registry::new(),
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        "",
        "tail",
    )
    .unwrap();
    assert!(String::from_utf8(c).unwrap().ends_with("tail\n"));
    // 多行保留、结尾换行归一
    let mut d = Vec::new();
    crate::overview::print_overview(
        &mut d,
        &reg,
        &test_words(),
        &Default::default(),
        Capabilities::default(),
        "a\nb\n\n\n",
        "",
    )
    .unwrap();
    let ds = String::from_utf8(d).unwrap();
    assert!(
        ds.starts_with(&format!("a\nb\n{}", crate::lang::t("overview.usage_line"))),
        "{ds:?}"
    );
}

#[test]
fn parse_serve_args_bare_flags() {
    let cfg = crate::builtins::parse_serve_args(
        &args(&[
            "--addr",
            ":9000",
            "--bearer=a,b",
            "--timeout=30s",
            "--cors",
            "x",
        ]),
        Config::default(),
    );
    assert_eq!(cfg.addr, ":9000");
    assert_eq!(cfg.bearer_tokens, vec!["a", "b"]);
    assert_eq!(cfg.timeout, std::time::Duration::from_secs(30));
    assert_eq!(cfg.cors_origins, vec!["x"]);
    // 缺省地址
    let cfg2 = crate::builtins::parse_serve_args(&[], Config::default());
    assert_eq!(cfg2.addr, ":8080");
}

#[test]
fn shadowing_modes_and_namespaced_reachability() {
    // spec §13.1：顶层段等于模式词的用户命令不再注册期报错，而是遮蔽裸词；
    // xyz.<词> 恒可达。
    let reg = test_reg(&["serve.x", "mcp.y"]);
    // 裸词让位：路由到用户命令（依赖 CLI 前端；无 cli 构建下走 stub）。
    #[cfg(feature = "cli")]
    {
        assert_eq!(
            run_config(&reg, args(&["serve", "x"]), Config::default()),
            0
        );
        assert_eq!(run_config(&reg, args(&["mcp", "y"]), Config::default()), 0);
    }
    // xyz.<词> 恒命中：模式帮助（不启动服务，任何构建都可）。
    assert_eq!(
        run_config(&reg, args(&["xyz.serve", "-h"]), Config::default()),
        0
    );
    assert_eq!(
        run_config(&reg, args(&["xyz.mcp", "-h"]), Config::default()),
        0
    );
    assert_eq!(
        run_config(&reg, args(&["xyz.http", "-h"]), Config::default()),
        0
    );
}

#[test]
fn cli_skipped_commands_do_not_shadow() {
    // spec §13.1：CLI-Skip 的命令不参与遮蔽——裸 mcp 仍是内建模式。
    let reg = Registry::new();
    Command::new("mcp.hidden", th)
        .cli(crate::spec::command::CliHints {
            skip: true,
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    assert_eq!(run_config(&reg, args(&["mcp", "-h"]), Config::default()), 0);
}

#[test]
fn help_subcommand_family() {
    // spec §10.4/§13.2：help → 总览；help <模式> → 模式帮助（不启动）；
    // help <命令路径>（点分或空格）→ 命令详细帮助。
    let reg = test_reg(&["user.add", "search.query"]);
    for argv in [
        vec!["help"],
        vec!["help", "serve"],
        vec!["help", "http"],
        vec!["help", "mcp"],
        vec!["help", "help"],
    ] {
        assert_eq!(
            run_config(&reg, args(&argv), Config::default()),
            0,
            "help family: {argv:?}"
        );
    }
    // 命令路径形态委托 CLI 的 -h；无 cli 构建下按 stub 语义（不计入断言）。
    #[cfg(feature = "cli")]
    for argv in [vec!["help", "user.add"], vec!["help", "user", "add"]] {
        assert_eq!(
            run_config(&reg, args(&argv), Config::default()),
            0,
            "help family: {argv:?}"
        );
    }
    // 别名路径等价（CLI 树解析别名）。
    #[cfg(feature = "cli")]
    {
        let reg2 = Registry::new();
        Command::new("user.add", th)
            .cli(crate::spec::command::CliHints {
                aliases: vec!["ua".into()],
                ..Default::default()
            })
            .register(&reg2)
            .unwrap();
        assert_eq!(
            run_config(&reg2, args(&["help", "ua"]), Config::default()),
            0
        );
    }
}

#[test]
fn mode_help_does_not_start_servers() {
    // serve/http/mcp 的 -h：打模式帮助 exit 0，不起服务。
    let reg = test_reg(&["user.add"]);
    for argv in [
        vec!["serve", "-h"],
        vec!["serve", "--help"],
        vec!["http", "-h"],
        vec!["mcp", "-h"],
    ] {
        assert_eq!(
            run_config(&reg, args(&argv), Config::default()),
            0,
            "mode -h: {argv:?}"
        );
    }
}

#[test]
fn shadowing_edges() {
    // 单段顶层名遮蔽：注册 "serve"（无点）同样让裸词归用户命令。
    let reg = test_reg(&["serve"]);
    #[cfg(feature = "cli")]
    assert_eq!(run_config(&reg, args(&["serve"]), Config::default()), 0);
    // xyz.<词> 仍达内建（模式帮助，不启动）。
    assert_eq!(
        run_config(&reg, args(&["xyz.serve", "-h"]), Config::default()),
        0
    );
    // help 家族与遮蔽交互：help serve → 用户命令帮助；help xyz.serve → 模式帮助。
    #[cfg(feature = "cli")]
    assert_eq!(
        run_config(&reg, args(&["help", "serve"]), Config::default()),
        0
    );
    assert_eq!(
        run_config(&reg, args(&["help", "xyz.serve"]), Config::default()),
        0
    );

    // 自定义模式词同样可被遮蔽（serve := httpd）。
    let cfg = Config {
        modes: ModeWords {
            serve: "httpd".into(),
            http: String::new(),
            mcp: String::new(),
            help: String::new(),
        },
        ..Default::default()
    };
    let reg2 = test_reg(&["httpd.x"]);
    #[cfg(feature = "cli")]
    assert_eq!(run_config(&reg2, args(&["httpd", "x"]), cfg.clone()), 0);
    assert_eq!(
        run_config(&reg2, args(&["xyz.httpd", "-h"]), cfg.clone()),
        0
    );
}

#[test]
fn four_mode_words_pairwise_distinct() {
    // 四词参与两两校验（spec §13.1），含 http 词。
    let cfg = Config {
        modes: ModeWords {
            serve: "d".into(),
            http: "d".into(),
            mcp: String::new(),
            help: String::new(),
        },
        ..Default::default()
    };
    assert_eq!(run_config(&test_reg(&["a.b"]), vec![], cfg), 2);
    let cfg2 = Config {
        modes: ModeWords {
            serve: String::new(),
            http: String::new(),
            mcp: "h".into(),
            help: "h".into(),
        },
        ..Default::default()
    };
    assert_eq!(run_config(&test_reg(&["a.b"]), vec![], cfg2), 2);
}
