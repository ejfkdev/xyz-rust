// spec 层测试：schema 形状、解码无损检查、默认值分层、校验规则、
// hints 合并与注册期报错。对齐 Go spec_test.go + hints_test.go。

use serde::Serialize;

use crate::Ctx;
use crate::XyzArgs;
use crate::XyzField;
use crate::XyzOutput;
use crate::errors::{self, Kind};
use crate::spec::Entry;
use crate::spec::command::{CliFieldHint, CliHints, Command, HTTPHints, MCPFieldHint, MCPHints};
use crate::spec::schema::schema_to_value;
use serde_json::{Map, Value, json};

fn invoke_args(e: &Entry, args: serde_json::Value) -> errors::Result<Value> {
    let obj = args.as_object().cloned().unwrap_or_default();
    (e.invoke)(&Ctx::new(), &obj)
}

// ---- schema 形状 ----

#[derive(XyzArgs)]
struct SchemaArgs {
    #[xyz(desc = "主旨", required)]
    name: String,
    #[xyz(desc = "计数", default = "18")]
    count: i32,
    #[xyz(desc = "模式", enum = "fast,slow", http = "query")]
    mode: String,
    #[xyz(skip, desc = "注入")]
    token: String,
    #[xyz(desc = "标签")]
    tags: Vec<String>,
    #[xyz(desc = "上限")]
    limit: Option<i64>,
    #[xyz(desc = "开关")]
    on: bool,
    #[xyz(desc = "比例")]
    ratio: f64,
}

fn schema_h(_: &Ctx, _: &SchemaArgs) -> errors::Result<Value> {
    Ok(json!(null))
}

#[test]
fn input_schema_shape() {
    let e = Command::new("t.schema", schema_h).entry().unwrap();
    let v = schema_to_value(&e.input_schema);
    assert_eq!(v["type"], "object");
    assert_eq!(v["properties"]["name"]["type"], "string");
    assert_eq!(v["properties"]["name"]["description"], "主旨");
    assert_eq!(v["properties"]["count"]["type"], "integer");
    assert_eq!(v["properties"]["count"]["default"], 18);
    assert_eq!(v["properties"]["mode"]["enum"].as_array().unwrap().len(), 2);
    assert_eq!(v["properties"]["on"]["type"], "boolean");
    assert_eq!(v["properties"]["ratio"]["type"], "number");
    assert_eq!(v["properties"]["tags"]["type"], "array");
    assert_eq!(v["properties"]["tags"]["items"]["type"], "string");
    assert_eq!(v["properties"]["limit"]["type"], "integer"); // Option 解引用透传
    assert!(v["properties"].get("token").is_none()); // skip 不进 schema
    let req = v["required"].as_array().unwrap();
    assert_eq!(req, &vec![json!("name")]);
    // 字段序 = 声明序（preserve_order）
    let props = v["properties"].as_object().unwrap();
    let keys: Vec<&String> = props.keys().collect();
    assert_eq!(
        keys,
        vec!["name", "count", "mode", "tags", "limit", "on", "ratio"]
    );
}

// ---- 解码与无损检查 ----

#[derive(XyzArgs)]
struct NumArgs {
    #[xyz(desc = "八位")]
    i8v: i8,
    #[xyz(desc = "无符号")]
    u64v: u64,
    #[xyz(desc = "浮点")]
    f: f64,
}

fn num_h(_: &Ctx, _: &NumArgs) -> errors::Result<Value> {
    Ok(json!(null))
}

#[test]
fn lossy_numeric_conversions_rejected() {
    let e = Command::new("t.num", num_h).entry().unwrap();
    // 3.7 永不静默变成 int
    let err = invoke_args(&e, json!({"i8v": 3.7})).unwrap_err();
    assert_eq!(errors::classify(&err), Some(Kind::InvalidInput));
    assert!(err.to_string().contains("expect integer"), "{err}");
    // 溢出
    let err = invoke_args(&e, json!({"i8v": 999})).unwrap_err();
    assert!(err.to_string().contains("overflows"), "{err}");
    // 无符号负数
    let err = invoke_args(&e, json!({"u64v": -1})).unwrap_err();
    assert!(err.to_string().contains("unsigned"), "{err}");
    // 字符串形态接受
    let out = invoke_args(&e, json!({"i8v": "7", "u64v": 1, "f": "2.5"})).unwrap();
    assert_eq!(out, json!(null));
}

#[derive(XyzArgs)]
struct StrForms {
    #[xyz(desc = "b")]
    b: bool,
    #[xyz(desc = "s")]
    s: String,
}

fn str_h(_: &Ctx, in_: &StrForms) -> errors::Result<Value> {
    Ok(json!({"b": in_.b, "s": in_.s}))
}

#[test]
fn string_forms_accepted() {
    let e = Command::new("t.str", str_h).entry().unwrap();
    let out = invoke_args(&e, json!({"b": "TRUE", "s": 42})).unwrap();
    assert_eq!(out, json!({"b": true, "s": "42"}));
    let err = invoke_args(&e, json!({"b": "yes"})).unwrap_err();
    assert!(err.to_string().contains("boolean"), "{err}");
}

// ---- 默认值分层 ----

#[derive(XyzArgs)]
struct DefaultsArgs {
    #[xyz(desc = "全局 18", default = "18")]
    age: i32,
    #[xyz(desc = "必填", required)]
    tag: String,
}

fn defaults_h(_: &Ctx, in_: &DefaultsArgs) -> errors::Result<Value> {
    Ok(json!({"age": in_.age, "tag": in_.tag}))
}

#[test]
fn interface_defaults_and_required() {
    let mut cli = CliHints::default();
    cli.fields.insert(
        "age".to_string(),
        CliFieldHint {
            default: Some(json!(20)),
            ..Default::default()
        },
    );
    let e = Command::new("t.def", defaults_h).cli(cli).entry().unwrap();
    // CLI 默认 20 注入后覆盖全局 18
    let mut m1: Map<String, Value> = e.cli_defaults();
    m1.insert("tag".to_string(), json!("x"));
    let out = (e.invoke)(&Ctx::new(), &m1).unwrap();
    assert_eq!(out, json!({"age": 20, "tag": "x"}));
    // 无覆盖时走全局 18
    let out2 = invoke_args(&e, json!({"tag": "y"})).unwrap();
    assert_eq!(out2, json!({"age": 18, "tag": "y"}));
    // 显式入参优先于 CLI 默认
    let mut m3: Map<String, Value> = e.cli_defaults();
    m3.insert("tag".to_string(), json!("z"));
    m3.insert("age".to_string(), json!(7));
    let out3 = (e.invoke)(&Ctx::new(), &m3).unwrap();
    assert_eq!(out3, json!({"age": 7, "tag": "z"}));
    // 必填缺失
    let err = invoke_args(&e, json!({})).unwrap_err();
    assert_eq!(errors::classify(&err), Some(Kind::InvalidInput));
    assert!(err.to_string().contains("is required"), "{err}");
}

// ---- MCP 默认值进 schema 但不覆盖显式入参 ----

#[derive(XyzArgs)]
struct McpArgs {
    #[xyz(desc = "默认 10", default = "10")]
    k: i32,
}

fn mcp_h(_: &Ctx, in_: &McpArgs) -> errors::Result<Value> {
    Ok(json!({"k": in_.k}))
}

#[test]
fn mcp_default_replaces_schema_default() {
    let mut mcp = MCPHints::default();
    mcp.fields.insert(
        "k".to_string(),
        MCPFieldHint {
            default: Some(json!(15)),
        },
    );
    let e = Command::new("t.mcp", mcp_h).mcp(mcp).entry().unwrap();
    // inputSchema 的 default = MCP 覆盖 15
    let v = schema_to_value(&e.input_schema);
    assert_eq!(v["properties"]["k"]["default"], 15);
    // MCP 前端只补缺席键（不覆盖显式入参）；Show 与 Go makeHandler 同责
    let out = (e.invoke)(&Ctx::new(), &e.mcp_defaults()).unwrap();
    assert_eq!(out, json!({"k": 15}));
    let mut explicit = e.mcp_defaults();
    explicit.insert("k".to_string(), json!(99));
    let out2 = (e.invoke)(&Ctx::new(), &explicit).unwrap();
    assert_eq!(out2, json!({"k": 99}));
}

// ---- skip 字段按 Rust 名注入 ----

#[derive(XyzArgs)]
struct InjectArgs {
    #[xyz(skip, secret, cli = "env=SECRET_X")]
    secret: String,
    #[xyz(desc = "v")]
    v: i32,
}

fn inject_h(_: &Ctx, in_: &InjectArgs) -> errors::Result<Value> {
    Ok(json!({"secret": in_.secret, "v": in_.v}))
}

#[test]
fn skip_field_injected_by_rust_name() {
    let e = Command::new("t.inj", inject_h).entry().unwrap();
    let out = invoke_args(&e, json!({"secret": "s3", "v": 1})).unwrap();
    assert_eq!(out, json!({"secret": "s3", "v": 1}));
}

// ---- 枚举 ----

#[derive(XyzArgs)]
struct EnumArgs {
    #[xyz(desc = "m", enum = "a,b")]
    m: String,
}

fn enum_h(_: &Ctx, _: &EnumArgs) -> errors::Result<Value> {
    Ok(json!(null))
}

#[test]
fn enum_membership_enforced() {
    let e = Command::new("t.enum", enum_h).entry().unwrap();
    assert!(invoke_args(&e, json!({"m": "a"})).is_ok());
    let err = invoke_args(&e, json!({"m": "c"})).unwrap_err();
    assert!(err.to_string().contains("one of"), "{err}");
}

// ---- 校验规则 ----

#[derive(XyzArgs)]
struct ValArgs {
    #[xyz(desc = "min2 max5", validate = "min=2,max=5")]
    s: String,
    #[xyz(desc = "email", validate = "omitempty,email")]
    mail: String,
    #[xyz(desc = "范围", validate = "gte=1,lte=10")]
    n: i32,
    #[xyz(desc = "oneof", validate = "oneof=a b")]
    w: String,
}

fn val_h(_: &Ctx, in_: &ValArgs) -> errors::Result<Value> {
    Ok(json!({"s": in_.s, "n": in_.n, "w": in_.w}))
}

#[test]
fn validation_rules() {
    let e = Command::new("t.val", val_h).entry().unwrap();
    let out = invoke_args(&e, json!({"s": "ab", "mail": "", "n": 5, "w": "a"})).unwrap();
    assert_eq!(out, json!({"s": "ab", "n": 5, "w": "a"}));
    let err = invoke_args(&e, json!({"s": "x", "n": 1, "w": "a"})).unwrap_err();
    assert_eq!(errors::classify(&err), Some(Kind::InvalidInput));
    let err = invoke_args(&e, json!({"s": "ab", "mail": "nope", "n": 1, "w": "a"})).unwrap_err();
    assert!(err.to_string().contains("email"), "{err}");
    let err = invoke_args(&e, json!({"s": "ab", "n": 1, "w": "z"})).unwrap_err();
    assert!(err.to_string().contains("oneof"), "{err}");
}

#[derive(XyzArgs)]
struct FracArgs {
    #[xyz(desc = "小数阈值", validate = "gt=1.5")]
    f: f64,
    #[xyz(desc = "整数阈值不变", validate = "min=2")]
    n: i64,
}

fn frac_h(_: &Ctx, in_: &FracArgs) -> errors::Result<Value> {
    Ok(json!({"f": in_.f, "n": in_.n}))
}

#[test]
fn fractional_thresholds_compare_in_float64() {
    // 上游 v0.2.2 修复：0.5/1.5 类阈值按浮点比较，不再被 int64 截断。
    let e = Command::new("t.frac", frac_h).entry().unwrap();
    // 1.4 不满足 gt=1.5（截断语义下 1==1 会误通过 min 类规则）
    let err = invoke_args(&e, json!({"f": 1.4, "n": 2})).unwrap_err();
    assert!(err.to_string().contains("gt"), "{err}");
    assert!(invoke_args(&e, json!({"f": 1.6, "n": 2})).is_ok());
    // 整数域行为不变
    let err = invoke_args(&e, json!({"f": 2.0, "n": 1})).unwrap_err();
    assert!(err.to_string().contains("min"), "{err}");
}

#[derive(XyzArgs)]
struct BadRule {
    #[xyz(desc = "x", validate = "explode=1")]
    x: String,
}

fn bad_rule_h(_: &Ctx, _: &BadRule) -> errors::Result<Value> {
    Ok(json!(null))
}

#[test]
fn unsupported_validate_rule_rejected_at_registration() {
    let err = Command::new("t.bad", bad_rule_h).entry().unwrap_err();
    assert!(
        err.to_string().contains("unsupported validate rule"),
        "{err}"
    );
}

// ---- hints 合并 ----

#[derive(XyzArgs)]
struct HintArgs {
    #[xyz(desc = "a", cli = "shorthand=a")]
    age: i32,
    #[xyz(desc = "n")]
    name: String,
}

fn hint_h(_: &Ctx, _: &HintArgs) -> errors::Result<Value> {
    Ok(json!(null))
}

#[test]
fn hints_merge_over_tags() {
    let mut cli = CliHints::default();
    cli.fields.insert(
        "age".to_string(),
        CliFieldHint {
            shorthand: Some("b".into()),
            default: Some(json!(5)),
            ..Default::default()
        },
    );
    cli.fields.insert(
        "NAME".to_string(),
        CliFieldHint {
            env_var: Some("N".into()),
            ..Default::default()
        },
    );
    let e = Command::new("t.hint", hint_h).cli(cli).entry().unwrap();
    let age = e
        .root
        .children
        .iter()
        .find(|f| f.json_name == "age")
        .unwrap();
    assert_eq!(age.cli.shorthand, Some('b')); // hint 覆盖属性
    assert_eq!(age.cli.default, Some(json!(5)));
    // Go 名忽略大小写匹配
    let name = e
        .root
        .children
        .iter()
        .find(|f| f.json_name == "name")
        .unwrap();
    assert_eq!(name.cli.env_var.as_deref(), Some("N"));
    // 未知字段在注册期报错
    let mut cli2 = CliHints::default();
    cli2.fields
        .insert("nope".to_string(), CliFieldHint::default());
    let err = Command::new("t.hint2", hint_h)
        .cli(cli2)
        .entry()
        .unwrap_err();
    assert!(err.to_string().contains("unknown field"), "{err}");
}

// ---- 嵌套 / 容器 ----

#[derive(XyzArgs)]
struct Nested {
    #[xyz(desc = "点数", required)]
    score: i32,
    #[xyz(desc = "备注")]
    note: String,
}

#[derive(XyzArgs)]
struct OuterArgs {
    #[xyz(desc = "内部")]
    inner: Nested,
    #[xyz(desc = "列表")]
    list: Vec<Nested>,
    #[xyz(desc = "可选")]
    maybe: Option<Nested>,
}

fn outer_h(_: &Ctx, in_: &OuterArgs) -> errors::Result<Value> {
    Ok(
        json!({"inner_score": in_.inner.score, "list_len": in_.list.len(), "maybe": in_.maybe.as_ref().map(|n| n.score)}),
    )
}

#[test]
fn nested_structs_and_slices() {
    let e = Command::new("t.nest", outer_h).entry().unwrap();
    let out = invoke_args(
        &e,
        json!({
            "inner": {"score": 9},
            "list": [{"score": 1}, {"score": 2}],
            "maybe": {"score": 3},
        }),
    )
    .unwrap();
    assert_eq!(out, json!({"inner_score": 9, "list_len": 2, "maybe": 3}));
    // 嵌套 required 递归校验：键缺席才触发（score=0 仍是「存在」）
    let err = invoke_args(&e, json!({"inner": {"note": "x"}, "list": []})).unwrap_err();
    assert!(err.to_string().contains("required"), "{err}");
    // Vec<struct> 元素递归校验
    let err = invoke_args(&e, json!({"inner": {"score": 1}, "list": [{"note": "x"}]})).unwrap_err();
    assert!(err.to_string().contains("required"), "{err}");
    // 未知键与 Go 一样被忽略（decodeTree 只认声明字段）
    let out = invoke_args(
        &e,
        json!({"inner": {"score": 1, "ghost": true}, "list": []}),
    )
    .unwrap();
    assert_eq!(out, json!({"inner_score": 1, "list_len": 0, "maybe": null}));
}

// ---- 枚举只支持标量（注册期报错） ----

#[derive(XyzArgs)]
struct BadEnum {
    #[xyz(desc = "x", enum = "a,b")]
    xs: Vec<String>,
}

fn bad_enum_h(_: &Ctx, _: &BadEnum) -> errors::Result<Value> {
    Ok(json!(null))
}

#[test]
fn enum_on_slice_rejected() {
    let err = Command::new("t.badenum", bad_enum_h).entry().unwrap_err();
    assert!(err.to_string().contains("scalar"), "{err}");
}

// ---- required 与 skip 冲突（宏期报错：compile_error，此处仅烟测可以共存） ----

// ---- 结果 schema（XyzOutput） ----

#[derive(Serialize, XyzOutput)]
#[xyz(desc = "用户响应")]
struct Resp {
    #[serde(rename = "id")]
    id: i64,
    #[serde(rename = "name")]
    name: String,
    #[xyz(required)]
    #[serde(rename = "tags")]
    tags: Vec<String>,
}

#[test]
fn output_schema_is_optional_and_serde_aware() {
    let http = HTTPHints {
        method: "GET".into(),
        path: "/x".into(),
        ..Default::default()
    };
    let e = Command::new("t.resp", |_: &Ctx, _: &EnumArgs| {
        Ok::<_, std::io::Error>(Resp {
            id: 1,
            name: "a".into(),
            tags: vec!["t".into()],
        })
    })
    .http(http)
    .entry()
    .unwrap();
    assert!(e.output_schema.is_some());
    let v = schema_to_value(e.output_schema.as_ref().unwrap());
    assert_eq!(v["type"], "object");
    assert_eq!(v["properties"]["id"]["type"], "integer");
    assert_eq!(v["properties"]["tags"]["type"], "array");
    assert_eq!(v["required"], json!(["tags"]));
}

// 结果类型无法 schematize 时 output_schema 为 None（本实现里所有支持类型
// 都有 schema；Vec<u8> 走 blanket 的 array 形态——README 差异节已记录）。

// ---- 命名标量 ----

#[derive(Clone, Copy, Debug, PartialEq, XyzField)]
struct Bulb(i8);

#[test]
fn named_scalar_roundtrip() {
    let v = json!(7);
    assert_eq!(Bulb::xyz_from_value(&v).unwrap().0, 7);
    assert!(Bulb::xyz_from_value(&json!("7")).is_ok());
    assert!(Bulb::xyz_from_value(&json!(7.5)).is_err());
}

// ---- 递归类型护栏 ----
// 自引用在宏期 compile_error、Box/&T 包装不被 XyzField 接受（编译期拒绝），
// 因此可达的递归入参在 Rust 里几乎不存在；运行时的 spec 深度护栏是防御
// 层，注册期经 catch_unwind 转错误。这里直接覆盖护栏单元语义。

#[test]
fn spec_depth_guard_panics_on_deep_nesting() {
    let result = std::panic::catch_unwind(|| {
        for _ in 0..50 {
            crate::spec::field::spec_depth_guard();
        }
    });
    assert!(result.is_err());
}

#[test]
fn tagged_union_schema_and_decode() {
    use crate::spec::field::FieldKind;
    use crate::spec::XyzField;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize, xyz_rust::XyzArgs)]
    #[serde(tag = "type")]
    enum Target {
        Element { state_id: i64, index: i64 },
        Coordinate {
            #[xyz(desc = "横坐标", required)]
            x: i64,
            y: i64,
        },
        Noop,
    }

    // 解码：tag 判别、恰好一分支；错误归 invalid_input。
    let e: Target =
        Target::xyz_from_value(&serde_json::json!({"type": "Element", "state_id": 9, "index": 1}))
            .unwrap();
    assert_eq!(e, Target::Element { state_id: 9, index: 1 });
    let c: Target = Target::xyz_from_value(&serde_json::json!({"type": "Coordinate", "x": 1, "y": 2}))
        .unwrap();
    assert_eq!(c, Target::Coordinate { x: 1, y: 2 });
    assert!(Target::xyz_from_value(&serde_json::json!({"type": "Ghost"})).is_err());
    assert!(Target::xyz_from_value(&serde_json::json!({"x": 1, "y": 2})).is_err());
    assert!(Target::xyz_from_value(&serde_json::json!({"type": "Element"})).is_err()); // 缺字段
    assert_eq!(
        Target::xyz_from_value(&serde_json::json!({"type": "Noop"})).unwrap(),
        Target::Noop
    );

    // 元数据：Union kind + 变体树；schema 层出 oneOf + const 判别。
    let meta = crate::spec::field::meta_from_spec(&Target::xyz_spec_of()).unwrap();
    assert!(matches!(meta.kind, FieldKind::Union));
    let union = meta.union.as_ref().expect("union tree");
    assert_eq!(union.tag, "type");
    assert_eq!(union.variants.len(), 3);
    assert_eq!(union.variants[0].name, "Element");
    assert_eq!(union.variants[0].meta.len(), 2);

    let union_schema = crate::spec::schema::field_schema(&meta);
    let sv = crate::spec::schema::schema_to_value(&union_schema);
    let one_of = sv["oneOf"].as_array().expect("oneOf array");
    assert_eq!(one_of.len(), 3);
    let branch = &one_of[1]; // Coordinate
    assert_eq!(branch["required"], serde_json::json!(["type", "x"]));
    assert_eq!(branch["properties"]["type"]["const"], "Coordinate");
    assert_eq!(branch["properties"]["type"]["type"], "string");
    assert_eq!(branch["properties"]["x"]["type"], "integer");
    assert_eq!(branch["properties"]["x"]["description"], "横坐标");
    // Noop 分支只有判别键
    assert_eq!(one_of[2]["required"], serde_json::json!(["type"]));
}

#[test]
fn tagged_union_into_entry_schema() {
    use crate::registry::Registry;
    use crate::spec::command::Command;
    use crate::spec::XyzArgs;
    use crate::Ctx;

    #[derive(serde::Serialize, serde::Deserialize, xyz_rust::XyzArgs)]
    #[serde(tag = "kind")]
    enum Sel {
        ById { id: i64 },
        ByName { name: String },
    }

    #[derive(xyz_rust::XyzArgs)]
    struct PickArgs {
        #[xyz(required, desc = "选择器")]
        sel: Sel,
    }

    fn pick(_: &Ctx, in_: &PickArgs) -> crate::errors::Result<String> {
        Ok(match &in_.sel {
            Sel::ById { id } => format!("id={id}"),
            Sel::ByName { name } => name.clone(),
        })
    }

    let reg = Registry::new();
    Command::new("demo.pick", pick)
        .register(&reg)
        .unwrap();
    let entry = reg.get("demo.pick").unwrap();
    let sch = crate::spec::schema::schema_to_value(&entry.input_schema);
    let sel = &sch["properties"]["sel"];
    assert_eq!(sel["oneOf"].as_array().unwrap().len(), 2);
    // 端到端 invoke：JSON 形态走 serde 判别。
    let out = (entry.invoke)(
        &Ctx::new(),
        &serde_json::from_str(r#"{"sel":{"kind":"ById","id":7}}"#).unwrap(),
    )
    .unwrap();
    assert_eq!(serde_json::to_value(out).unwrap(), serde_json::json!("id=7"));
}
