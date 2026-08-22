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
                mcp: "serve".into(),
                help: String::new(),
            },
            ..Default::default()
        },
        Config {
            modes: ModeWords {
                serve: "-serve".into(),
                mcp: String::new(),
                help: String::new(),
            },
            ..Default::default()
        },
        Config {
            modes: ModeWords {
                serve: "sv c".into(),
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
fn run_reserved_names() {
    for name in ["serve.x", "mcp.up", "help.me"] {
        assert_eq!(run(&test_reg(&[name]), args(&["whatever"])), 2);
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
fn overview_help_blocks() {
    // 通过 print_overview 直接断言块插入与归一化
    let reg = test_reg(&["a.b"]);
    let mut buf = Vec::new();
    let before = "myapp v1.2.3 — do the thing\nhttps://github.com/me/myapp";
    let after = "Need help? https://github.com/me/myapp#faq";
    crate::overview::print_overview(
        &mut buf,
        &reg,
        "serve",
        "mcp",
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
        "serve",
        "mcp",
        Capabilities::default(),
        "",
        "",
    )
    .unwrap();
    crate::overview::print_overview(
        &mut b,
        &reg,
        "serve",
        "mcp",
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
        "serve",
        "mcp",
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
        "serve",
        "mcp",
        Capabilities::default(),
        "a\nb\n\n\n",
        "",
    )
    .unwrap();
    let ds = String::from_utf8(d).unwrap();
    assert!(ds.starts_with("a\nb\n用法"), "{ds:?}");
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
