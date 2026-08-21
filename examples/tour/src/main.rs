// 教学导览：展示三通道绑定、默认值分层和 schema 生成的内部视图。
// 与 examples/example（真实二进制形态）互补。Run with: cargo run -p xyz-tour
//
// Go 原版：cmd/tour/main.go。显式 Registry + spec::command::Command::new
// （而不是 define 链）：注册、取回 Entry、打印输入 schema 与字段绑定总览、
// 模拟三种前端注入通道默认值后走同一条 Invoke 管线，最后演示错误分类。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::Serialize;
use xyz_rust::errs;
use xyz_rust::serde_json::{Map, Value};
use xyz_rust::spec::JsonMap;
use xyz_rust::spec::command::Command;
use xyz_rust::{
    CliFieldHint, CliHints, Ctx, Entry, HTTPFieldHint, HTTPHints, MCPHints, XyzArgs, XyzOutput,
};

// AddUserArgs 是唯一的入参定义。
// 全局契约（所有通道生效）放 tag：desc/default/required/enum/validate/secret。
// 通道绑定也可以放 cli/http 属性，或在定义时用 CliHints.Fields /
// HTTPHints.Fields / MCPHints.Fields 覆盖。

#[derive(XyzArgs)]
struct AddUserArgs {
    #[xyz(
        desc = "用户名称",
        required,
        validate = "min=2",
        cli = "positional",
        http = "path"
    )]
    name: String,
    #[xyz(desc = "年龄", default = "18", http = "query")]
    age: i32,
    #[xyz(
        desc = "部署模式",
        enum = "fast,slow",
        cli = "shorthand=m",
        http = "query"
    )]
    mode: String,
    #[xyz(desc = "标签", http = "query")]
    tags: Vec<String>,
    #[xyz(desc = "返回条数上限", http = "query")]
    limit: Option<i64>,
    #[xyz(skip, secret, desc = "令牌", cli = "env=ACM_TOKEN")]
    token: String,
}

#[derive(Serialize, XyzOutput)]
struct AddUserResp {
    id: i64,
    name: String,
    age: i32,
    mode: String,
}

static ID_COUNTER: AtomicI64 = AtomicI64::new(0);

fn add_user(_: &Ctx, in_: &AddUserArgs) -> errs::Result<AddUserResp> {
    if in_.name == "missing" {
        return Err(errs::new(errs::Kind::NotFound, "no such user"));
    }
    let id = ID_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    Ok(AddUserResp {
        id,
        name: in_.name.clone(),
        age: in_.age,
        mode: in_.mode.clone(),
    })
}

#[derive(XyzArgs)]
struct SearchArgs {
    #[xyz(desc = "关键词", required)]
    query: String,
    #[xyz(desc = "返回条数", default = "10")]
    k: i32,
    #[xyz(desc = "过滤标签")]
    tags: Vec<String>,
}

fn search(_: &Ctx, in_: &SearchArgs) -> errs::Result<Vec<String>> {
    Ok(vec![
        in_.query.clone(),
        "...".to_string(),
        format!("top {}", in_.k),
    ])
}

fn main() -> errs::Result<()> {
    let reg = xyz_rust::Registry::new();

    // —— 定义一次：三种通道的配置集中在这里 ——
    let user_entry = Command::new("user.add", add_user)
        .summary("创建用户")
        .cli(CliHints {
            usage: "add <name>".into(),
            fields: HashMap::from([
                (
                    // CLI 专属默认值（覆盖全局 18）
                    "age".to_string(),
                    CliFieldHint {
                        shorthand: Some("a".into()),
                        default: Some(20.into()),
                        ..Default::default()
                    },
                ),
                (
                    // 只有 CLI 才有这个默认值
                    "mode".to_string(),
                    CliFieldHint {
                        default: Some("fast".into()),
                        ..Default::default()
                    },
                ),
            ]),
            ..Default::default()
        })
        .http(HTTPHints {
            method: "POST".into(),
            path: "/users".into(),
            fields: HashMap::from([(
                // 覆盖全局默认 18，只对 HTTP 生效
                "age".to_string(),
                HTTPFieldHint {
                    default: Some(21.into()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        })
        .mcp(MCPHints {
            annotations: vec!["write".into()],
            ..Default::default()
        })
        .register(&reg)?;

    let search_entry = Command::new("search.query", search)
        .summary("搜索文档")
        .cli(CliHints {
            fields: HashMap::from([(
                // CLI 覆盖全局默认 10
                "k".to_string(),
                CliFieldHint {
                    default: Some(25.into()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        })
        .http(HTTPHints {
            method: "GET".into(),
            path: "/search".into(),
            ..Default::default()
        })
        .register(&reg)?;

    let ctx = Ctx::new();

    println!("== 注册的命令 ==");
    for n in reg.names() {
        let e = reg.get(&n).expect("name just listed");
        println!("  {:<14} {}", n, e.summary);
    }

    println!(
        "\n== {} 的 JSON Schema（这是 MCP 的契约；OpenAPI 以后也吃它） ==",
        user_entry.name
    );
    println!(
        "{}",
        xyz_rust::serde_json::to_string_pretty(&user_entry.input_schema)
            .expect("input schema serializes")
    );

    println!("\n== 每个字段在三个通道的绑定与默认值总览 ==");
    print_fields(&user_entry);
    print_fields(&search_entry);

    println!("\n== 三种运行形态，各注入自己的通道默认值后走同一条管线 ==");
    println!("模拟 CLI 前端：flag 解析字符串 + 注入 CLIDefaults");
    show(
        &ctx,
        &user_entry,
        merge(&user_entry.cli_defaults(), &map(&[("name", "bob".into())])),
    );
    println!("模拟 HTTP 前端：query 解析 + 注入 HTTPDefaults（age 21 覆盖全局 18）");
    show(
        &ctx,
        &user_entry,
        merge(
            &user_entry.http_defaults(),
            &map(&[("name", "curie".into())]),
        ),
    );
    println!("模拟 MCP 前端：按 schema 直接传参（没有通道默认值注入）");
    show(&ctx, &user_entry, map(&[("name", "ada".into())]));

    println!("\n== 搜索：全局默认 k=10，CLI 覆盖 k=25 ==");
    show(&ctx, &search_entry, map(&[("query", "golang".into())]));
    show(
        &ctx,
        &search_entry,
        merge(
            &search_entry.cli_defaults(),
            &map(&[("query", "go".into())]),
        ),
    );

    println!("\n== 缺必填字段：统一归为 invalid_input ==");
    match (user_entry.invoke)(&ctx, &Map::new()) {
        Ok(_) => println!("  ok"),
        Err(err) => show_err(err),
    }

    println!("\n== 业务错误：not_found → HTTP 404 / JSON-RPC -32001 / CLI 退出码 1 ==");
    match (user_entry.invoke)(&ctx, &map(&[("name", "missing".into())])) {
        Ok(_) => println!("  ok"),
        Err(err) => show_err(err),
    }

    Ok(())
}

// print_fields 展示解析后的元数据：一次定义产出了每个通道能直接消费的信息。
fn print_fields(e: &Entry) {
    println!("  [{}]", e.name);
    for f in &e.root.children {
        if f.skip {
            continue;
        }
        println!(
            "    {:<8} CLI{{短名:{} env:{} positional:{} 默认:{}}} HTTP{{位置:{} 默认:{}}} MCP{{默认:{}}} 全局默认:{}",
            f.json_name,
            or_dash_char(f.cli.shorthand),
            or_dash_opt(f.cli.env_var.as_deref()),
            f.cli.positional,
            or_nil(f.cli.default.as_ref()),
            or_dash(&f.http.location),
            or_nil(f.http.default.as_ref()),
            or_nil(f.mcp.default.as_ref()),
            or_nil(f.default.as_ref()),
        );
    }
}

fn show(ctx: &Ctx, e: &Entry, args: JsonMap) {
    match (e.invoke)(ctx, &args) {
        Ok(out) => println!(
            "  ok  -> {}",
            xyz_rust::serde_json::to_string(&out).expect("value serializes")
        ),
        Err(err) => show_err(err),
    }
}

// merge 是通道前端的职责缩略版：通道默认值先铺底，用户提供值覆盖。
fn merge(defaults: &JsonMap, provided: &JsonMap) -> JsonMap {
    let mut out = defaults.clone();
    out.extend(provided.iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

fn map(pairs: &[(&str, Value)]) -> JsonMap {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn show_err(err: xyz_rust::Error) {
    let kind = kind_of(&err);
    println!("  err  -> {err}");
    println!(
        "  kind -> {kind} | HTTP {} | exit {} | JSON-RPC {}",
        errs::http_status(kind),
        errs::exit_code(kind),
        errs::jsonrpc_code(kind)
    );
}

/// 对齐 Go 的 errors.As 语义：沿 source 链取最内层带分类的 errors::Error。
/// Rust 的 Invoke 把 handler 错误用 errors::Error::upgrade 包了一层（外层
/// Kind::Internal，cause 链保留原分类），直接 errs::classify 会在第一层就
/// 返回 Internal；穿链到最内层才拿回 handler 的 not_found。
fn kind_of(err: &(dyn std::error::Error + 'static)) -> errs::Kind {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    let mut last = errs::Kind::Internal;
    while let Some(e) = cur {
        if let Some(ce) = e.downcast_ref::<xyz_rust::Error>() {
            last = ce.kind();
        }
        cur = e.source();
    }
    last
}

fn or_dash(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.to_string()
    }
}

fn or_dash_char(c: Option<char>) -> String {
    c.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string())
}

fn or_dash_opt(s: Option<&str>) -> String {
    match s {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "-".to_string(),
    }
}

fn or_nil(v: Option<&Value>) -> String {
    match v {
        Some(v) => xyz_rust::cli::render::format_cell(v),
        None => "-".to_string(),
    }
}
