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
            format: None,
            // 这些测试断言人类可读（text）输出；注入 writer 按规范是非
            // 交互的（auto→jsonl），故显式声明交互式裁定（§10.7）。
            interactive: Some(true),
            ..Default::default()
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
fn cjk_padding_matches_go_width_semantics() {
    // Go：宽度按字节计（len()）、填充按字符计（%-*s）；CJK 键需逐字节一致。
    let mut buf = Vec::new();
    render_value(&mut buf, &serde_json::json!({"\u{540d}\u{5b57}": "v"})).unwrap();
    let out = String::from_utf8(buf).unwrap();
    // "名字" 2 字符 6 字节 → 补 4 空格到 6 字符，再接双空格槽位
    assert_eq!(out, "\u{540d}\u{5b57}      v\n");
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
fn default_subcommand_forwards_all_args() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct ExtractArgs {
        #[xyz(desc = "归档", required, cli = "positional")]
        name: String,
        #[xyz(desc = "级别", default = "0")]
        level: i32,
    }
    fn extract(_: &Ctx, in_: &ExtractArgs) -> errors::Result<String> {
        Ok(format!("{}:{}", in_.name, in_.level))
    }
    Command::new("extract", extract)
        .cli(crate::CliHints {
            usage: "extract <name>".into(),
            default: true,
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    // 老用法：udf ./image.tar —— 全部参数转发给默认的 extract
    let (code, out, _) = run_app(&reg, &["./image.tar", "--level", "9"]);
    assert_eq!(code, 0);
    assert_eq!(out, "./image.tar:9\n");
    // 显式子命令路径不受影响
    let (code, out, _) = run_app(&reg, &["extract", "tarball.tar"]);
    assert_eq!(code, 0);
    assert_eq!(out, "tarball.tar:0\n");
    // -h/-v 不触发默认下沉（根帮助仍在）
    let (code, out, _) = run_app(&reg, &["-h"]);
    assert_eq!(code, 0);
    assert!(out.contains("Usage:"), "{out}");
    assert!(out.contains("extract"), "{out}");
    // 位置参数之后的 -h 归默认命令自己的帮助
    let (code, out, _) = run_app(&reg, &["img.tar", "-h"]);
    assert_eq!(code, 0);
    assert!(out.contains("extract <name> [flags]"), "{out}");
}

#[test]
fn help_blocks_on_leaf_only() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct E {
        #[xyz(desc = "x")]
        x: String,
    }
    fn eh(_: &Ctx, _: &E) -> errors::Result<String> {
        Ok(String::new())
    }
    Command::new("extract", eh)
        .cli(crate::CliHints {
            before: "extract — 解包镜像\n用法示例见下方".into(),
            after: "更多: https://example.com/udf#extract".into(),
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    let (code, out, _) = run_app(&reg, &["extract", "-h"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("extract — 解包镜像\n"), "{out}");
    assert!(
        out.ends_with("更多: https://example.com/udf#extract\n"),
        "{out}"
    );
    // 中间节点帮助不出现叶子块
    let reg2 = Registry::new();
    Command::new("user.add", eh)
        .cli(crate::CliHints {
            before: "LEAFBLK".into(),
            ..Default::default()
        })
        .register(&reg2)
        .unwrap();
    let (_, out2, _) = run_app(&reg2, &["user", "-h"]);
    assert!(!out2.contains("LEAFBLK"), "{out2}");
    let (code3, out3, _) = run_app(&reg2, &["user", "add", "-h"]);
    assert_eq!(code3, 0);
    assert!(out3.starts_with("LEAFBLK\n"), "{out3}");
}

#[test]
fn duplicate_default_rejected_at_build() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct X {
        #[xyz(desc = "x")]
        x: String,
    }
    fn xh(_: &Ctx, _: &X) -> errors::Result<String> {
        Ok(String::new())
    }
    Command::new("a.one", xh)
        .cli(crate::CliHints {
            default: true,
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    Command::new("a.two", xh)
        .cli(crate::CliHints {
            default: true,
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    let err = app_build_error(&reg);
    assert!(err.to_string().contains("default conflicts"), "{err}");
}

#[test]
fn double_dash_terminator_guards_v_and_json() {
    let reg = Registry::new();

    #[derive(XyzArgs)]
    struct P {
        #[xyz(desc = "a", required, cli = "positional")]
        a: String,
    }
    fn ph(_: &Ctx, in_: &P) -> errors::Result<String> {
        Ok(in_.a.clone())
    }
    Command::new("user.add", ph).register(&reg).unwrap();
    // "--" 之后全是位置参数：-v / --json 不再是开关
    let (code, out, _) = run_app(&reg, &["user", "add", "--", "-v"]);
    assert_eq!(code, 0);
    assert_eq!(out, "-v\n");
    let (code, out, _) = run_app(&reg, &["user", "add", "--", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(out, "--json\n");
    // 未带 -- 时版本照旧
    let (code, _, _) = run_app(&reg, &["-v"]);
    assert_eq!(code, 0);
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
    assert!(err.contains("positional argument count mismatch"), "{err}");
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

#[test]
fn block_envelope_spills_binary_to_files() {
    let env = serde_json::json!({"content":[
        {"type":"text","text":"hello"},
        {"type":"image","mimeType":"image/png","data":"aGVsbG8="}
    ]});
    let mut buf = Vec::new();
    crate::cli::render::render_value(&mut buf, &env).unwrap();
    let out = String::from_utf8(buf).unwrap();
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("hello"));
    let path = lines.next().unwrap().trim();
    assert!(path.contains("xyz-blk-"), "path = {path}");
    assert!(path.ends_with(".png"), "path = {path}");
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(bytes, b"hello");
    std::fs::remove_file(path).ok();
}

#[test]
fn union_field_degrades_not_fatals() {
    use crate::Ctx;
    use crate::registry::Registry;
    use crate::spec::command::Command;

    #[derive(serde::Serialize, serde::Deserialize, xyz_rust::XyzArgs)]
    #[serde(tag = "type")]
    enum Sel {
        ById { id: i64 },
        ByName { name: String },
    }

    #[derive(xyz_rust::XyzArgs)]
    struct PickArgs {
        /// 联合字段：CLI 策略为跳过；可缺席用 Option<Sel>。
        sel: Option<Sel>,
        #[xyz(desc = "标记", required)]
        tag_note: String,
    }

    fn pick(_: &Ctx, in_: &PickArgs) -> crate::errors::Result<String> {
        Ok(format!(
            "{}/{}",
            in_.tag_note,
            in_.sel.as_ref().map(|_| "sel").unwrap_or("no-sel")
        ))
    }

    let reg = Registry::new();
    Command::new("demo.pick", pick).register(&reg).unwrap();
    // Bug B 实锤修复：含联合字段的命令不再把整棵 CLI 树弄挂——
    // App 构建成功，无联合字段照常调度，联合字段本身静默跳过。
    let entry = reg.get("demo.pick").unwrap();
    let names: Vec<String> = entry
        .root
        .children
        .iter()
        .map(|c| format!("{}:{}", c.json_name, c.kind.schema_type()))
        .collect();
    let (_hcode, hout, _herr) = run_app(&reg, &["demo", "pick", "-h"]);
    let (code, _out, err) = run_app(&reg, &["demo", "pick", "--tag_note", "hi"]);
    assert_eq!(
        code, 0,
        "command with union field must run (union skipped); stderr: {err}; help: {hout}; fields: {names:?}"
    );
}

#[test]
fn format_dispatch_and_conflict_yield() {
    use crate::Ctx;
    use crate::registry::Registry;
    use crate::spec::command::Command;

    #[derive(xyz_rust::XyzArgs)]
    struct FArgs {
        #[xyz(desc = "n")]
        n: i64,
    }
    fn list(_: &Ctx, _: &FArgs) -> crate::errors::Result<Vec<i64>> {
        Ok(vec![1, 2])
    }
    let reg = Registry::new();
    Command::new("f.list", list).register(&reg).unwrap();

    // 无冲突命令：裸 --format jsonl 生效（数组逐元素紧凑一行）。
    let (code, out, err) = run_app(&reg, &["f", "list", "--format", "jsonl", "--n", "1"]);
    assert_eq!(code, 0, "err: {err}");
    assert_eq!(out, "1\n2\n");

    let (code, out, _) = run_app(&reg, &["f", "list", "--format=markdown", "--n", "1"]);
    assert_eq!(code, 0);
    assert_eq!(out, "- 1\n- 2\n");

    // --xyz.format 的默认格式经 Options 注入（派发器同款路径在这里用手工
    // Options 模拟）：裸标志仍是最高优先级。
    let out_buf = Buf::default();
    let err_buf = Buf::default();
    let mut a = App::new_with_options(
        &reg,
        Options {
            out: Some(Box::new(out_buf.clone())),
            err_out: Some(Box::new(err_buf.clone())),
            format: Some("json".into()),
            interactive: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let argv: Vec<String> = ["f", "list", "--n", "1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(a.run(&argv), 0);
    assert!(
        out_buf.text().trim_start().starts_with('['),
        "{}",
        out_buf.text()
    );

    // 非法格式：用法错误 exit 2。
    let (code, _o, e) = run_app(&reg, &["f", "list", "--format", "bogus"]);
    assert_eq!(code, 2);
    assert!(e.contains("invalid output format"), "{e}");

    // 机器格式下命令错误走 §8.6 错误体（stderr，非纯文本行）。
    #[derive(xyz_rust::XyzArgs)]
    struct BoomArgs {
        #[xyz(desc = "n")]
        n: i64,
    }
    fn boom(_: &Ctx, _: &BoomArgs) -> crate::errors::Result<String> {
        Err(
            crate::errors::Error::new(crate::errors::Kind::NotFound, "gone")
                .with_code("GONE")
                .with_detail("id", serde_json::json!(7)),
        )
    }
    Command::new("f.boom", boom).register(&reg).unwrap();
    let (code, _o, err) = run_app(&reg, &["f", "boom", "--format", "json", "--n", "1"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"], "gone");
    assert_eq!(v["kind"], "not_found");
    assert_eq!(v["code"], "GONE");
    assert_eq!(v["detail"]["id"], 7);
    // text 格式仍是人类可读一行。
    let (code, _o, err) = run_app(&reg, &["f", "boom", "--n", "1"]);
    assert_eq!(code, 1);
    assert!(!err.trim_start().starts_with('{'), "{err}");
}

#[test]
fn format_flag_yields_to_command_field() {
    use crate::Ctx;
    use crate::registry::Registry;
    use crate::spec::command::Command;

    // 命令自有 format 字段：裸 --format 归命令（全局格式只认 --xyz.format）。
    #[derive(xyz_rust::XyzArgs)]
    struct HasFormatArgs {
        #[xyz(desc = "fmt")]
        format: String,
    }
    fn echo(_: &Ctx, a: &HasFormatArgs) -> crate::errors::Result<String> {
        Ok(a.format.clone())
    }
    let reg = Registry::new();
    Command::new("g.echo", echo).register(&reg).unwrap();
    let (code, out, err) = run_app(&reg, &["g", "echo", "--format", "json"]);
    assert_eq!(code, 0, "err: {err}");
    // 未让位的话会被当全局格式并吃掉参数（命令将报缺参）；这里必须原样到手。
    assert_eq!(out.trim(), "json");
}

#[test]
fn format_auto_tty_resolution() {
    use crate::Ctx;
    use crate::registry::Registry;
    use crate::spec::command::Command;

    #[derive(xyz_rust::XyzArgs)]
    struct AArgs {
        #[xyz(desc = "n")]
        n: i64,
    }
    #[derive(xyz_rust::XyzArgs)]
    struct HArgs {
        #[xyz(desc = "n")]
        n: i64,
    }
    fn list(_: &Ctx, _: &AArgs) -> crate::errors::Result<Vec<i64>> {
        Ok(vec![1, 2])
    }
    fn hint_md(_: &Ctx, _: &HArgs) -> crate::errors::Result<Vec<i64>> {
        Ok(vec![3])
    }
    let reg = Registry::new();
    Command::new("a.list", list).register(&reg).unwrap();
    // 层 3：逐命令 hint。
    Command::new("a.md", hint_md)
        .cli(crate::spec::command::CliHints {
            format: "markdown".into(),
            ..Default::default()
        })
        .register(&reg)
        .unwrap();

    let run_with = |opts: Options, args: &[&str]| -> (i32, String, String) {
        let out = Buf::default();
        let err = Buf::default();
        let mut a = App::new_with_options(
            &reg,
            Options {
                out: Some(Box::new(out.clone())),
                err_out: Some(Box::new(err.clone())),
                ..opts
            },
        )
        .unwrap();
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let code = a.run(&argv);
        (code, out.text(), err.text())
    };

    // 内建默认 auto：交互式 → text；非交互 → jsonl（spec §10.7）。
    let (code, out, _) = run_with(
        Options {
            interactive: Some(true),
            ..Default::default()
        },
        &["a", "list", "--n", "1"],
    );
    assert_eq!(code, 0);
    assert_eq!(out, "1\n2\n"); // text 渲染（标量每行一个）
    let (_, out, _) = run_with(
        Options {
            interactive: Some(false),
            ..Default::default()
        },
        &["a", "list", "--n", "1"],
    );
    assert_eq!(out, "1\n2\n"); // jsonl：数组逐元素 —— 数字形态一致

    // 两半可配置：交互 → markdown；非交互 → jsonl（默认）。
    let (_, out, _) = run_with(
        Options {
            interactive: Some(true),
            format_interactive: Some("markdown".into()),
            ..Default::default()
        },
        &["a", "list", "--n", "1"],
    );
    assert_eq!(out, "- 1\n- 2\n");

    // 层 3 hint 在非交互下直通（markdown 具体值）。
    let (_, out, _) = run_with(
        Options {
            interactive: Some(false),
            ..Default::default()
        },
        &["a", "md", "--n", "1"],
    );
    assert_eq!(out, "- 3\n");

    // 层 4 代码全局（format=json 具体值）压过内建 auto；层 1 命令行压过层 4。
    let (_, out, _) = run_with(
        Options {
            interactive: Some(false),
            format: Some("json".into()),
            ..Default::default()
        },
        &["a", "list", "--n", "1"],
    );
    assert!(out.trim_start().starts_with('['), "{out}");
    let (_, out, _) = run_with(
        Options {
            interactive: Some(false),
            format: Some("json".into()),
            ..Default::default()
        },
        &["a", "list", "--n", "1", "--format", "jsonl"],
    );
    assert_eq!(out, "1\n2\n");
}

#[test]
fn ctx_language_and_env_api() {
    use crate::Ctx;
    // 进程级访问器（spec §14 item7）。
    assert!(!xyz_rust::language().is_empty());
    let e1 = xyz_rust::env();
    assert_eq!(e1.language, xyz_rust::language());
    assert_eq!(e1.no_color, xyz_rust::no_color());
    // 逐请求语言载体（§11.7）：with_language + language_from_ctx。
    let ctx = Ctx::new();
    assert_eq!(xyz_rust::language_from_ctx(&ctx), xyz_rust::language());
    let zh = ctx.with_language("zh-CN");
    assert_eq!(xyz_rust::language_from_ctx(&zh), "zh-CN");
    assert_eq!(zh.language(), Some("zh-CN"));
    // 取消信号不受语言派生影响。
    zh.cancel();
    assert!(zh.cancelled() && ctx.cancelled());
}

#[test]
fn format_tier_interactions_and_auto_errors() {
    use crate::Ctx;
    use crate::registry::Registry;
    use crate::spec::command::Command;

    #[derive(xyz_rust::XyzArgs)]
    struct LArgs {
        #[xyz(desc = "n")]
        n: i64,
    }
    fn list(_: &Ctx, _: &LArgs) -> crate::errors::Result<Vec<i64>> {
        Ok(vec![1, 2])
    }
    fn hint_bad(_: &Ctx, _: &LArgs) -> crate::errors::Result<String> {
        Ok("S".to_string())
    }
    fn boom(_: &Ctx, _: &LArgs) -> crate::errors::Result<String> {
        Err(crate::errors::Error::new(
            crate::errors::Kind::NotFound,
            "gone",
        ))
    }
    let reg = Registry::new();
    Command::new("b.list", list).register(&reg).unwrap();
    Command::new("b.bad", hint_bad)
        .cli(crate::spec::command::CliHints {
            format: "bogus".into(), // 非法 hint：宽松忽略、落下层
            ..Default::default()
        })
        .register(&reg)
        .unwrap();
    Command::new("b.boom", boom).register(&reg).unwrap();

    let run_with = |opts: Options, args: &[&str]| -> (i32, String, String) {
        let out = Buf::default();
        let err = Buf::default();
        let mut a = App::new_with_options(
            &reg,
            Options {
                out: Some(Box::new(out.clone())),
                err_out: Some(Box::new(err.clone())),
                ..opts
            },
        )
        .unwrap();
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let code = a.run(&argv);
        (code, out.text(), err.text())
    };

    // 显式 bare --format auto 也走 TTY 解析（两半）。
    let (_, out, _) = run_with(
        Options {
            interactive: Some(true),
            ..Default::default()
        },
        &["b", "list", "--n", "1", "--format", "auto"],
    );
    assert_eq!(out, "1\n2\n"); // text
    let (_, out, _) = run_with(
        Options {
            interactive: Some(false),
            format_interactive: Some("markdown".into()),
            ..Default::default()
        },
        &["b", "list", "--n", "1", "--format", "auto"],
    );
    assert_eq!(out, "1\n2\n"); // auto→piped 默认 jsonl（数字形态与 text 同行）

    // 层 1 > 层 2：裸 --format 压过 --xyz.format（命令行全局）。
    let (_, out, _) = run_with(
        Options {
            interactive: Some(true),
            format: Some("jsonl".into()),
            format_from_flag: true,
            ..Default::default()
        },
        &["b", "list", "--n", "1", "--format", "json"],
    );
    assert!(out.trim_start().starts_with('['), "{out}"); // pretty JSON 数组

    // 层 2 > 层 3：--xyz.format（命令行）压过逐命令 hint（jsonl 输出带引号）。
    let (_, out, _) = run_with(
        Options {
            interactive: Some(true),
            format: Some("jsonl".into()),
            format_from_flag: true,
            ..Default::default()
        },
        &["b", "bad", "--n", "1"],
    );
    assert_eq!(out, "\"S\"\n");

    // 层 3 非法值：读到值即停，渲染落 text（对齐 Go resolveFormat/default）。
    let (_, out, _) = run_with(
        Options {
            interactive: Some(false),
            ..Default::default()
        },
        &["b", "bad", "--n", "1"],
    );
    assert_eq!(out, "S\n");

    // 层 4 非法（代码配置）→ 构建期报错 exit 2（对齐 Go NewWithOptions）。
    let code = crate::cli::run_context(
        &Ctx::new(),
        &reg,
        &[
            "b".to_string(),
            "list".to_string(),
            "--n".to_string(),
            "1".to_string(),
        ],
        Options {
            interactive: Some(false),
            format: Some("bogus".into()),
            ..Default::default()
        },
    );
    assert_eq!(code, 2);

    // auto→jsonl 下命令错误：stderr 是紧凑单行 §8.6 体。
    let (code, _o, err) = run_with(
        Options {
            interactive: Some(false),
            ..Default::default()
        },
        &["b", "boom", "--n", "1"],
    );
    assert_eq!(code, 1);
    let trimmed = err.trim();
    assert!(trimmed.starts_with('{'), "{err}");
    assert!(!trimmed.contains('\n'), "compact single line: {err}");
    let v: serde_json::Value = serde_json::from_str(trimmed).unwrap();
    assert_eq!(v["kind"], "not_found");
}
