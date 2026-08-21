// cli 前端集成测试：渲染、App 级行为、帮助、补全。对齐 Go cli_test.go。

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use xyz_rust::XyzArgs;

use crate::Ctx;
use crate::cli::render::{format_cell, render_value};
use crate::cli::{App, Options};
use crate::errors;
use crate::registry::Registry;
use crate::spec::command::Command;

/// 共享缓冲写入器：App 写入、测试读取，绕过 dyn 下转。
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

#[derive(XyzArgs)]
struct EchoArgs {
    #[xyz(desc = "内容", required)]
    s: String,
    #[xyz(desc = "次数", default = "1")]
    n: i32,
}

fn echo(_: &Ctx, in_: &EchoArgs) -> errors::Result<String> {
    Ok(in_.s.repeat(in_.n.max(1) as usize))
}

fn run_app(reg: &Registry, args: &[&str]) -> (i32, String, String) {
    let out = Buf::default();
    let err = Buf::default();
    let mut a = App::new_with_options(
        reg,
        Options {
            out: Some(Box::new(out.clone())),
            err_out: Some(Box::new(err.clone())),
        },
    )
    .unwrap();
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let code = a.run(&argv);
    (code, out.text(), err.text())
}

/// App::new 错误（无 Debug，不清求 unwrap_err）。
fn app_build_error(reg: &Registry) -> errors::Error {
    match App::new(reg) {
        Ok(_) => panic!("expected build error"),
        Err(e) => e,
    }
}

fn echo_reg() -> Registry {
    let reg = Registry::new();
    Command::new("echo.hi", echo).register(&reg).unwrap();
    reg
}

#[test]
fn render_table_aligned() {
    let v: Value = serde_json::to_value(vec![
        serde_json::json!({"id": 1, "name": "a"}),
        serde_json::json!({"id": 22, "name": "bb"}),
    ])
    .unwrap();
    let mut buf = Vec::new();
    render_value(&mut buf, &v).unwrap();
    let out = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "id  name");
    assert_eq!(lines[1], "--  ----");
    assert_eq!(lines[2], "1   a   ");
    assert_eq!(lines[3], "22  bb  ");
}

#[test]
fn render_scalars_and_kvs() {
    let mut buf = Vec::new();
    render_value(&mut buf, &Value::Null).unwrap();
    assert!(buf.is_empty());
    render_value(&mut buf, &serde_json::json!("hi")).unwrap();
    render_value(&mut buf, &serde_json::json!(5)).unwrap();
    render_value(&mut buf, &serde_json::json!(true)).unwrap();
    render_value(&mut buf, &serde_json::json!(2.5)).unwrap();
    render_value(&mut buf, &serde_json::json!([1, 2])).unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out, "hi\n5\ntrue\n2.5\n1\n2\n");
    // float 裸值对齐 Go %v："3" 而非 "3.0"
    let mut buf2 = Vec::new();
    render_value(&mut buf2, &serde_json::json!(3.0)).unwrap();
    assert_eq!(String::from_utf8(buf2).unwrap(), "3\n");
    // KV 对齐
    let mut buf3 = Vec::new();
    render_value(&mut buf3, &serde_json::json!({"k": "v", "long": 1})).unwrap();
    assert_eq!(String::from_utf8(buf3).unwrap(), "k     v\nlong  1\n");
}

#[test]
fn format_cell_variants() {
    assert_eq!(format_cell(&Value::Null), "");
    assert_eq!(format_cell(&serde_json::json!([1, "a"])), "[1 a]");
    assert_eq!(format_cell(&serde_json::json!({"a": 1})), "{\"a\":1}");
}

#[test]
fn render_generic_serialize() {
    let mut buf = Vec::new();
    crate::cli::render(&mut buf, &serde_json::json!({"z": 1, "a": "x"})).unwrap();
    // preserve_order：声明序（z 在 a 前）
    assert_eq!(String::from_utf8(buf).unwrap(), "z  1\na  x\n");
}

#[test]
fn app_echo_success() {
    let reg = echo_reg();
    let (code, out, _) = run_app(&reg, &["echo", "hi", "--s", "ab", "--n", "2"]);
    assert_eq!(code, 0);
    assert_eq!(out, "abab\n");
}

#[test]
fn app_missing_required_exits_2() {
    let reg = echo_reg();
    let (code, _, err) = run_app(&reg, &["echo", "hi"]);
    assert_eq!(code, 2);
    assert!(err.contains("required"), "{err}");
}

#[test]
fn app_unknown_flag_exits_2() {
    let reg = echo_reg();
    let (code, _, err) = run_app(&reg, &["echo", "hi", "--ghost"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown flag: --ghost"), "{err}");
}

#[test]
fn app_unknown_command_prints_help() {
    let reg = echo_reg();
    let (code, out, _) = run_app(&reg, &["echo"]);
    assert_eq!(code, 0); // 中间节点打印帮助后正常退出
    assert!(out.contains("Usage:"), "{out}");
}

#[test]
fn app_version_flag() {
    let reg = echo_reg();
    let (code, out, _) = run_app(&reg, &["echo", "hi", "-v"]);
    assert_eq!(code, 0);
    assert!(out.contains("version"), "{out}");
}

#[test]
fn app_json_flag() {
    let reg = echo_reg();
    let (code, out, _) = run_app(&reg, &["echo", "hi", "--s", "x", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(out, "\"x\"\n");
}

#[test]
fn completion_bash_contains_words() {
    let reg = echo_reg();
    let (code, out, _) = run_app(&reg, &["completion", "bash"]);
    assert_eq!(code, 0);
    assert!(out.contains("compgen -W"), "{out}");
    assert!(out.contains("echo"), "{out}");
}

#[test]
fn completion_unknown_shell_exits_2() {
    let reg = echo_reg();
    let (code, _, _) = run_app(&reg, &["completion", "tcsh"]);
    assert_eq!(code, 2);
}

#[derive(XyzArgs)]
struct AliasArgs {
    #[xyz(desc = "v1", cli = "positional")]
    a: String,
}

fn alias_h(_: &Ctx, in_: &AliasArgs) -> errors::Result<String> {
    Ok(in_.a.clone())
}

#[test]
fn aliases_resolve_like_subcommands() {
    let reg = Registry::new();
    Command::new("user.add", alias_h)
        .cli(crate::CliHints {
            usage: "add <a>".into(),
            aliases: vec!["ua".into()],
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    let (code, out, _) = run_app(&reg, &["user", "ua", "x"]);
    assert_eq!(code, 0);
    assert_eq!(out, "x\n");
}

#[test]
fn conflicting_alias_rejected_at_build() {
    let reg = Registry::new();
    Command::new("a.one", alias_h).register(&reg).unwrap();
    Command::new("a.two", alias_h)
        .cli(crate::CliHints {
            aliases: vec!["one".into()],
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    let err = app_build_error(&reg);
    assert!(err.to_string().contains("alias"), "{err}");
}

#[test]
fn optional_after_required_positional_rejected() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct P {
        #[xyz(desc = "a", cli = "positional")]
        a: String,
        #[xyz(desc = "b", required, cli = "positional")]
        b: String,
    }
    fn ph(_: &Ctx, _: &P) -> errors::Result<String> {
        Ok(String::new())
    }
    Command::new("p.x", ph).register(&reg).unwrap();
    let err = app_build_error(&reg);
    assert!(
        err.to_string().contains("must not follow optional"),
        "{err}"
    );
}

#[test]
fn nested_struct_field_rejected_for_cli() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct Sub {
        #[xyz(desc = "x")]
        x: i32,
    }
    #[derive(XyzArgs)]
    struct Top {
        #[xyz(desc = "s")]
        sub: Sub,
    }
    fn th(_: &Ctx, _: &Top) -> errors::Result<String> {
        Ok(String::new())
    }
    Command::new("t.x", th).register(&reg).unwrap();
    let err = app_build_error(&reg);
    assert!(err.to_string().contains("not supported"), "{err}");
}

#[test]
fn middleware_chain_sees_args_and_can_short_circuit() {
    let reg = echo_reg();
    let mut a = App::new(&reg).unwrap();
    use crate::cli::ExecContext;
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen2 = std::sync::Arc::clone(&seen);
    a.use_mw(Box::new(move |_ctx, ec: &ExecContext, args, next| {
        assert_eq!(ec.path, "echo.hi");
        assert!(args.contains_key("s"));
        seen2.store(true, std::sync::atomic::Ordering::Relaxed);
        next(args) // 续链
    }));
    let argv: Vec<String> = ["echo", "hi", "--s", "x"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let code = a.run(&argv);
    assert_eq!(code, 0);
    assert!(seen.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn positional_min_max() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct Pos {
        #[xyz(desc = "a", required, cli = "positional")]
        a: String,
        #[xyz(desc = "b", cli = "positional")]
        b: String,
    }
    fn ph(_: &Ctx, in_: &Pos) -> errors::Result<String> {
        Ok(format!("{}:{}", in_.a, in_.b))
    }
    Command::new("p.x", ph).register(&reg).unwrap();
    let (code, out, _) = run_app(&reg, &["p", "x", "one", "two"]);
    assert_eq!(code, 0);
    assert_eq!(out, "one:two\n");
    let (code, _, err) = run_app(&reg, &["p", "x"]);
    assert_eq!(code, 2);
    assert!(err.contains("位置参数数量不符"), "{err}");
}

#[derive(Serialize, XyzArgs)]
struct RowOut {
    #[xyz(desc = "编号")]
    id: i64,
    #[xyz(desc = "名")]
    name: String,
}

fn _table(_: &Ctx, _: &EchoArgs) -> errors::Result<Vec<RowOut>> {
    Ok(vec![])
}

#[test]
fn unused_serialize_bound_smoke() {
    // R 类型推导：Vec<RowOut> 同时满足 Serialize + XyzSchema（XyzArgs 派生
    // 自带 XyzSchema 实现）。
    fn f(_: &Ctx, _: &EchoArgs) -> errors::Result<Vec<RowOut>> {
        Ok(vec![RowOut {
            id: 1,
            name: "a".into(),
        }])
    }
    let e = Command::new("rows.list", f).entry().unwrap();
    assert!(e.output_schema.is_some());
}
