// 完整示例：一条 xyz_rust::define(...)...run() 链 = 整个程序。
// 覆盖常用写法：位置参数、短名/别名、env 注入与 secret 字段、枚举、
// 指针与切片、命名标量、Duration/Vec<u8>、required/validate、
// 全局默认值与 CLI/HTTP/MCP 三层覆盖、表格输出、基础类型返回、
// 错误分类（not_found/invalid_input/unauthorized）、MCP 注解。
//
// 试用：
//
//	cargo run --example example -- user add bob -a 20 -m fast --tags a,b
//	APP_TOKEN=t0ken cargo run --example example -- user add alice --verbose
//	cargo run --example example -- user ua carol            # 别名
//	cargo run --example example -- user rm alice            # 成功；user rm bob → not_found
//	cargo run --example example -- user list                # []struct → 表格
//	cargo run --example example -- search query golang      # CLI 专属默认 k=25
//	cargo run --example example -- math sum --a 1 --b 2     # 基础类型 i64
//	cargo run --example example -- time now                 # 时间值（RFC3339）
//	cargo run --example example -- sys sleep --d 300ms      # std Duration；>5s 报错
//	cargo run --example example -- sys port -p 9090         # 命名标量 #[derive(XyzField)]
//	cargo run --example example -- file hash --data hello   # Vec<u8>（SHA-256）
//	NEXT_KEY=k cargo run --example example -- net head      # header/http_name + env
//	cargo run --example example -- mcp stdio                # MCP：命令即工具
//	cargo run --example example -- serve --addr :8080       # HTTP：REST + /openapi.json + /mcp

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use xyz_rust::errs;
use xyz_rust::{CliFieldHint, CliHints, HTTPHints, MCPHints, XyzArgs, XyzField, XyzOutput};

// ---- user.add：最完整的定义——三个通道的配置全在这里 ----

#[derive(XyzArgs)]
struct AddUserArgs {
    #[xyz(
        desc = "用户名称",
        required,
        validate = "min=2,max=32",
        cli = "positional",
        http = "path"
    )]
    name: String,
    #[xyz(desc = "邮箱", validate = "omitempty,email")]
    email: String,
    #[xyz(desc = "年龄", default = "18", http = "query")]
    age: i32,
    #[xyz(desc = "部署模式", enum = "fast,slow", http = "query")]
    mode: String,
    #[xyz(desc = "标签", http = "query")]
    tags: Vec<String>,
    #[xyz(desc = "分页上限", http = "query")]
    limit: Option<i64>,
    #[xyz(skip, secret, desc = "API 令牌（仅 env 注入，不进 schema）")]
    token: String,
    #[xyz(desc = "打印详细信息")]
    verbose: bool,
}

#[derive(Serialize, XyzOutput)]
struct AddUserResp {
    #[serde(rename = "id")]
    id: i64,
    #[serde(rename = "name")]
    name: String,
    #[serde(rename = "age")]
    age: i32,
    #[serde(rename = "token_set")]
    token_set: bool,
}

static ID_COUNTER: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn add_user(_ctx: &xyz_rust::Ctx, in_: &AddUserArgs) -> errs::Result<AddUserResp> {
    if in_.name == "missing" {
        return Err(errs::new(errs::Kind::NotFound, "no such user"));
    }
    let id = ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    Ok(AddUserResp {
        id,
        name: in_.name.clone(),
        age: in_.age,
        token_set: !in_.token.is_empty(),
    })
}

// ---- user.list / user.rm ----

#[derive(XyzArgs)]
struct ListArgs {}

fn list_users(_ctx: &xyz_rust::Ctx, _in_: &ListArgs) -> errs::Result<Vec<AddUserResp>> {
    Ok(vec![
        AddUserResp {
            id: 1,
            name: "alice".into(),
            age: 18,
            token_set: false,
        },
        AddUserResp {
            id: 2,
            name: "bob".into(),
            age: 25,
            token_set: false,
        },
    ])
}

#[derive(XyzArgs)]
struct RmArgs {
    #[xyz(desc = "要删除的用户", required, cli = "positional")]
    name: String,
    #[xyz(desc = "强制删除（隐藏 flag）", cli = "hidden")]
    force: bool,
}

fn rm_user(_ctx: &xyz_rust::Ctx, in_: &RmArgs) -> errs::Result<String> {
    if in_.name != "alice" {
        return Err(errs::new(
            errs::Kind::NotFound,
            format!("user {:?} not found", in_.name),
        ));
    }
    Ok(if in_.force {
        "user alice removed (forced)".to_string()
    } else {
        "user alice removed".to_string()
    })
}

// ---- search.query：三层默认值（全局 10 / CLI 25 / MCP 15） ----

#[derive(XyzArgs)]
struct SearchArgs {
    #[xyz(desc = "关键词", required)]
    query: String,
    #[xyz(desc = "返回条数", default = "10")]
    k: i32,
    #[xyz(desc = "过滤标签")]
    tags: Vec<String>,
}

fn search(_ctx: &xyz_rust::Ctx, in_: &SearchArgs) -> errs::Result<Vec<String>> {
    Ok(vec![
        in_.query.clone(),
        "...".to_string(),
        format!("top {}", in_.k),
    ])
}

// ---- math.sum / math.div：required 标量与基础类型返回 ----

#[derive(XyzArgs)]
struct SumArgs {
    #[xyz(desc = "左操作数", required)]
    a: i64,
    #[xyz(desc = "右操作数", required)]
    b: i64,
}

fn sum(_ctx: &xyz_rust::Ctx, in_: &SumArgs) -> errs::Result<i64> {
    Ok(in_.a + in_.b)
}

#[derive(XyzArgs)]
struct DivArgs {
    #[xyz(desc = "被除数", required)]
    a: f64,
    #[xyz(desc = "除数", required)]
    b: f64,
}

fn div(_ctx: &xyz_rust::Ctx, in_: &DivArgs) -> errs::Result<f64> {
    if in_.b == 0.0 {
        return Err(errs::new(errs::Kind::InvalidInput, "divisor is zero"));
    }
    Ok(in_.a / in_.b)
}

// ---- time.now ----

#[derive(XyzArgs)]
struct ClockArgs {}

fn now(_ctx: &xyz_rust::Ctx, _in_: &ClockArgs) -> errs::Result<DateTime<Utc>> {
    Ok(Utc::now())
}

// ---- sys.sleep：std Duration 入参 ----

#[derive(XyzArgs)]
struct SleepArgs {
    #[xyz(desc = "睡眠时长", default = "100ms")]
    d: Duration,
}

fn sleep(_ctx: &xyz_rust::Ctx, in_: &SleepArgs) -> errs::Result<String> {
    if in_.d > Duration::from_secs(5) {
        return Err(errs::new(
            errs::Kind::InvalidInput,
            "sleep too long (max 5s)",
        ));
    }
    std::thread::sleep(in_.d);
    Ok(format!("slept {}", xyz_rust::spec::fmt_duration(in_.d)))
}

// ---- sys.port：命名标量类型 ----

#[derive(Clone, Copy, Debug, PartialEq, XyzField)]
struct Port(i32);

#[derive(XyzArgs)]
struct PortArgs {
    #[xyz(desc = "监听端口", default = "8080", cli = "shorthand=p")]
    port: Port,
}

fn listen(_ctx: &xyz_rust::Ctx, in_: &PortArgs) -> errs::Result<String> {
    Ok(format!("listening on :{}", in_.port.0))
}

// ---- file.hash：Vec<u8> 入参 ----

#[derive(XyzArgs)]
struct HashArgs {
    #[xyz(desc = "原始内容", required)]
    data: Vec<u8>,
}

fn hash_data(_ctx: &xyz_rust::Ctx, in_: &HashArgs) -> errs::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&in_.data);
    Ok(format!("{:x}", hasher.finalize()))
}

// ---- net.head：HTTP header 绑定 + http_name + secret + env ----

#[derive(XyzArgs)]
struct HeadArgs {
    #[xyz(
        skip,
        secret,
        desc = "API Key（header/env 注入）",
        http = "header",
        http_name = "X-Api-Key",
        cli = "env=NEXT_KEY"
    )]
    key: String,
}

fn head(_ctx: &xyz_rust::Ctx, in_: &HeadArgs) -> errs::Result<String> {
    if in_.key.is_empty() {
        return Err(errs::new(
            errs::Kind::Unauthorized,
            "missing X-Api-Key (set env NEXT_KEY)",
        ));
    }
    Ok(format!("api key accepted ({} bytes)", in_.key.len()))
}

fn cli_fields(pairs: &[(&str, CliFieldHint)]) -> HashMap<String, CliFieldHint> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn main() {
    xyz_rust::define("user.add", add_user)
        .summary("创建用户")
        .description(
            "创建新用户并返回内部 ID。\n三种通道的全部配置都在这一个定义里：位置参数、短名、别名、env 注入的 secret、枚举、指针与切片、校验规则。",
        )
        .cli(CliHints {
            usage: "add <name>".into(),
            aliases: vec!["ua".into(), "new".into()],
            fields: cli_fields(&[
                ("age", CliFieldHint { shorthand: Some("a".into()), default: Some(20.into()), ..Default::default() }),
                ("mode", CliFieldHint { shorthand: Some("m".into()), ..Default::default() }),
                ("tags", CliFieldHint { shorthand: Some("t".into()), ..Default::default() }),
                ("token", CliFieldHint { env_var: Some("APP_TOKEN".into()), ..Default::default() }),
                ("verbose", CliFieldHint { shorthand: Some("V".into()), ..Default::default() }),
            ]),
            ..Default::default()
        })
        .http(HTTPHints { method: "POST".into(), path: "/users/{name}".into(), ..Default::default() })
        .mcp(MCPHints { annotations: vec!["write".into(), "title:创建用户".into()], ..Default::default() })
        .also(&[
            &xyz_rust::define("user.list", list_users)
                .summary("列出用户")
                .description("返回用户切片：CLI 渲染成对齐表格，结构化通道保持原始数组。")
                .http(HTTPHints { method: "GET".into(), path: "/users".into(), ..Default::default() }),

            &xyz_rust::define("user.rm", rm_user)
                .summary("删除用户")
                .cli(CliHints { usage: "rm <name>".into(), aliases: vec!["del".into()], ..Default::default() })
                .mcp(MCPHints { annotations: vec!["destructive".into()], ..Default::default() }),

            &xyz_rust::define("search.query", search)
                .summary("搜索文档")
                .cli(CliHints {
                    fields: cli_fields(&[("k", CliFieldHint { default: Some(25.into()), ..Default::default() })]),
                    ..Default::default()
                })
                .http(HTTPHints { method: "GET".into(), path: "/search".into(), ..Default::default() })
                .mcp(MCPHints {
                    fields: HashMap::from([(
                        "k".to_string(),
                        xyz_rust::MCPFieldHint { default: Some(15.into()) },
                    )]),
                    annotations: Vec::new(),
                }),

            &xyz_rust::define("math.sum", sum)
                .summary("两数求和")
                .mcp(MCPHints { annotations: vec!["read".into(), "idempotent".into()], ..Default::default() }),

            &xyz_rust::define("math.div", div).summary("两数相除"),

            &xyz_rust::define("time.now", now).summary("当前 UTC 时间"),

            &xyz_rust::define("sys.sleep", sleep)
                .summary("睡眠指定时长")
                .cli(CliHints {
                    fields: cli_fields(&[("d", CliFieldHint { shorthand: Some("d".into()), ..Default::default() })]),
                    ..Default::default()
                }),

            &xyz_rust::define("sys.port", listen)
                .summary("监听端口")
                .cli(CliHints {
                    fields: cli_fields(&[("port", CliFieldHint { shorthand: Some("p".into()), ..Default::default() })]),
                    ..Default::default()
                }),

            &xyz_rust::define("file.hash", hash_data)
                .summary("计算 SHA-256")
                .cli(CliHints {
                    fields: cli_fields(&[("data", CliFieldHint { shorthand: Some("d".into()), ..Default::default() })]),
                    ..Default::default()
                }),

            &xyz_rust::define("net.head", head)
                .summary("探测 API Key 注入")
                .http(HTTPHints { method: "GET".into(), path: "/headers".into(), ..Default::default() }),
        ])
        // 可选的能力开关示例：
        //   .configure(xyz_rust::Config { capabilities: xyz_rust::Capabilities { no_mcp: true, ..Default::default() }, ..Default::default() })
        .run(); // 注册全部命令、派发并按结果退出
}
