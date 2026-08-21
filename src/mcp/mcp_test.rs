// mcp 前端逻辑层测试（Go mcp_test.go 的单元面；线上往返已在 example
// stdio 冒烟验证——rmcp 3.1.4 无进程内 transport）。覆盖：版本校验、
// 参数解析、注解解析、工具元数据/inputSchema/outputSchema、默认值语义。

use std::sync::Arc;

use rmcp::handler::server::ServerHandler;

use crate::Ctx;
use crate::errors;
use crate::mcp::{DEFAULT_VERSIONS, Options, handler, transport};
use crate::registry::Registry;
use crate::spec::command::{Command, MCPFieldHint, MCPHints};
use xyz_rust::XyzArgs;

#[derive(XyzArgs)]
struct SumArgs {
    #[xyz(desc = "左操作数", required)]
    a: i64,
    #[xyz(desc = "右操作数", required)]
    b: i64,
    #[xyz(desc = "全局 10", default = "10")]
    k: i32,
}

#[derive(serde::Serialize, xyz_rust::XyzOutput)]
struct SumResp {
    #[serde(rename = "sum")]
    sum: i64,
}

fn sum(_: &Ctx, in_: &SumArgs) -> errors::Result<SumResp> {
    Ok(SumResp { sum: in_.a + in_.b })
}

fn reg() -> Registry {
    let reg = Registry::new();
    Command::new("math.sum", sum)
        .summary("求和")
        .description("两个整数相加")
        .mcp(MCPHints {
            annotations: vec!["read".into(), "title:求和工具".into(), "destructive".into()],
            fields: std::collections::HashMap::from([(
                "k".to_string(),
                MCPFieldHint {
                    default: Some(15.into()),
                },
            )]),
        })
        .register(&reg)
        .unwrap();
    reg
}

#[test]
fn versions_validation() {
    assert!(transport::validate_versions(&[]).is_ok());
    assert!(transport::validate_versions(&["2025-11-25".into()]).is_ok());
    assert!(transport::validate_versions(&["2026-07-28".into()]).is_ok());
    let err = transport::validate_versions(&["1999-01-01".into()]).unwrap_err();
    assert!(
        err.to_string().contains("unknown protocol version"),
        "{err}"
    );
    let err = transport::validate_versions(&["".into()]).unwrap_err();
    assert!(err.to_string().contains("empty protocol version"), "{err}");
}

#[test]
fn transport_version_constraints() {
    // streamable HTTP：2026-07-28 需无状态模式（与 SDK 内部约束一致）
    let mut opts = Options {
        versions: vec![crate::mcp::PROTOCOL_V2026_07_28.into()],
        ..Default::default()
    };
    assert!(transport::validate_transport_versions("http", &opts).is_err());
    opts.stateless = true;
    assert!(transport::validate_transport_versions("http", &opts).is_ok());
    // stdio 全版本
    let mut s_opts = Options {
        versions: vec![crate::mcp::PROTOCOL_V2025_03_26.into()],
        ..Default::default()
    };
    assert!(transport::validate_transport_versions("stdio", &s_opts).is_ok());
    s_opts.versions = vec![];
    assert!(transport::validate_transport_versions("http", &s_opts).is_ok());
}

#[test]
fn default_versions_newest_first() {
    assert_eq!(DEFAULT_VERSIONS[0], "2026-07-28");
    assert_eq!(DEFAULT_VERSIONS.len(), 5);
}

#[test]
fn build_creates_tool_metadata() {
    let mut opts = Options::default();
    // 钉定→supported_protocol_versions 收窄
    let srv = handler::build(&reg(), &opts, Arc::new(Ctx::new())).unwrap();
    let tool = srv.get_tool("math.sum").unwrap();
    assert_eq!(tool.name, "math.sum");
    assert_eq!(tool.description.as_deref(), Some("求和\n\n两个整数相加"));
    // inputSchema：MCP 默认 15 替换全局 10
    let inschema = tool.input_schema.clone();
    let v: serde_json::Value = serde_json::json!(serde_json::to_value(&*inschema).unwrap());
    assert_eq!(v["properties"]["k"]["default"], 15);
    assert_eq!(v["properties"]["a"]["type"], "integer");
    assert_eq!(v["required"], serde_json::json!(["a", "b"]));
    // outputSchema 同源反射
    let out = tool.output_schema.as_ref().unwrap();
    let ov: serde_json::Value = serde_json::to_value(&**out).unwrap();
    assert_eq!(ov["properties"]["sum"]["type"], "integer");
    // 注解：read → read_only_hint=true；destructive；title
    let ann = tool.annotations.as_ref().unwrap();
    assert_eq!(ann.read_only_hint, Some(true));
    assert_eq!(ann.destructive_hint, Some(true));
    assert_eq!(ann.title.as_deref(), Some("求和工具"));

    // 钉定子集反映在 supported_protocol_versions
    opts.versions = vec!["2025-11-25".into()];
    let pinned = handler::build(&reg(), &opts, Arc::new(Ctx::new())).unwrap();
    let supported = pinned.supported_protocol_versions().into_owned();
    assert_eq!(supported.len(), 1);
    assert_eq!(supported[0].as_str(), "2025-11-25");
}

#[test]
fn impl_defaults() {
    let srv = handler::build(&reg(), &Options::default(), Arc::new(Ctx::new())).unwrap();
    let info = srv.get_info();
    assert_eq!(info.server_info.name, crate::cli::app::bin_name());
    assert_eq!(info.server_info.version, "0.0.0");
}

#[test]
fn parse_args_forms() {
    let (t, o) = crate::mcp::args::parse_args(&s2(&[
        "http",
        "--addr",
        ":9000",
        "--versions=a,b",
        "--json-response",
        "--stateless",
        "--bearer=x",
        "--session-timeout",
        "90s",
        "--name",
        "N",
        "--server-version",
        "V",
    ]))
    .unwrap();
    assert_eq!(t, "http");
    assert_eq!(o.addr, ":9000");
    assert_eq!(o.versions, vec!["a", "b"]);
    assert!(o.json_response);
    assert!(o.stateless);
    assert_eq!(o.bearer_tokens, vec!["x"]);
    assert_eq!(o.session_timeout, std::time::Duration::from_secs(90));
    assert_eq!(o.name, "N");
    assert_eq!(o.version, "V");

    let err = crate::mcp::args::parse_args(&s2(&[])).unwrap_err();
    assert!(err.to_string().contains("missing transport"), "{err}");
    let err = crate::mcp::args::parse_args(&s2(&["x", "y"])).unwrap_err();
    assert!(err.to_string().contains("unexpected argument"), "{err}");
    let err = crate::mcp::args::parse_args(&s2(&["--ghost", "stdio"])).unwrap_err();
    assert!(err.to_string().contains("unknown flag"), "{err}");
}

#[test]
fn options_merge_defaults() {
    let base = Options {
        addr: ":9000".into(),
        bearer_tokens: vec!["b".into()],
        stateless: true,
        ..Default::default()
    };
    let mut from_flags = Options::default();
    from_flags.merge_defaults(&base);
    assert_eq!(from_flags.addr, ":9000");
    assert_eq!(from_flags.bearer_tokens, vec!["b"]);
    assert!(from_flags.stateless);
    // 命令行 flag 优先
    let mut from_flags2 = Options {
        addr: ":7777".into(),
        ..Default::default()
    };
    from_flags2.merge_defaults(&base);
    assert_eq!(from_flags2.addr, ":7777");
}

fn s2(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}
