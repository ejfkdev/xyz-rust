# xyz-rust — 一次定义，三通道调用

> 语言 / Language: [English](README.md) · **中文（当前页）**

[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![MCP Protocol](https://img.shields.io/badge/MCP-2024%E2%80%932026--07--28-0764e0?style=flat)](https://modelcontextprotocol.io/specification/2026-07-28)
[![Dependencies](https://img.shields.io/badge/核心-std%2Bserde%2Fchrono-2ea44f?style=flat)](#依赖原则与体积)

[xyz-go](https://github.com/ejfkdev/xyz-go) 的 Rust 移植版：**只定义一次**命令（入参结构 + 校验 + 各通道细节），同一个二进制自动获得 **CLI 子命令**、**HTTP REST 服务**（附 OpenAPI 文档）与 **MCP 工具服务**（官方 Rust SDK）三种运行形态，模式由库自动判断。

```rust
use xyz_rust::errs;
use xyz_rust::{define, CliHints, HTTPHints, MCPHints, XyzArgs};

#[derive(XyzArgs)]
struct AddArgs {
    #[xyz(desc = "用户名", required, validate = "min=2,max=32", cli = "positional", http = "path")]
    name: String,
    #[xyz(desc = "年龄", default = "18")]
    age: i32,
}

fn add(_ctx: &xyz_rust::Ctx, in_: &AddArgs) -> errs::Result<String> {
    Ok(format!("{} is {}", in_.name, in_.age))
}

fn main() {
    define("user.add", add)
        .summary("添加用户")
        .cli(CliHints { usage: "add <name>".into(), ..Default::default() })
        .http(HTTPHints { method: "POST".into(), path: "/users/{name}".into(), ..Default::default() })
        .mcp(MCPHints { annotations: vec!["write".into()], ..Default::default() })
        .run(); // 注册 + 派发 + 按结果退出，到此为止
}
```

```console
$ cargo run -p xyz-example -- user add bob -a 20    # CLI：子命令 + 短名 flag
id          1
name        bob
age         20
token_set   false

$ cargo run -p xyz-example -- user list             # Vec<struct> 渲染成对齐表格
id  name   age  token_set
--  -----  ---  ---------
1   alice  18   false
2   bob    25   false

$ curl -s -X POST localhost:8080/users/alice -d '{"age":9}'
{"id":1,"name":"alice","age":9,"token_set":false}

$ cargo run -p xyz-example -- math sum --a 1 --b 2  # 基础类型裸输出一行
3

$ cargo run -p xyz-example -- mcp stdio            # MCP：注册的命令即工具（stdio/streamable HTTP）
```

## 特性

- **一次定义，零样板**：整个 `main` 就是一条 `define(...) ... .run()` 链，无需 `std::process::exit`、无需手动构造注册表、无需分发开关。
- **三通道共用一条管线**：CLI（字符串）、HTTP（JSON）、MCP arguments 先归约成同一个 `serde_json` map，再走「解码 → 默认值 → 校验 → handler」的唯一路径，行为不漂移。
- **逐通道精细配置**：短名、别名、env 注入、绑定位置、**通道专属默认值**（全局属性默认 → 通道覆盖两级分层）。
- **无信封响应**：基础类型裸输出、struct 键值对齐、`Vec<struct>` 表格、`--json` 翻转；HTTP 裸 JSON；MCP `structuredContent` + `textContent` 双份。
- **统一错误分类**：一条 `errs::new(errs::Kind::NotFound, ...)` 同时驱动 CLI 退出码、HTTP 状态码、MCP 错误码。
- **依赖洁癖**：核心模块（spec/registry/errors/cli/logx/根）除 serde 家族与 chrono 外**零第三方依赖**；唯一的其他第三方树是官方 Rust SDK（rmcp），可用 `--no-default-features` 整体剔除；按通道裁剪后最小约 0.81M。
- **协议版本可控**：MCP 支持 2024-11-05 至 2026-07-28 五个规范版本，`--versions` 一键限定；工具同时携带派生宏生成的 `outputSchema`（OpenAPI 响应 schema 同源）。
- **生产友好**：SIGINT/SIGTERM 优雅关停（`Ctx` 贯穿到 handler）、`/healthz` 探活、gzip、CLI 帮助内联默认值/env/枚举、`completion bash|zsh|fish`。
- **内置参数开箱即用**：凭据（`--bearer`）、默认地址、日志级别、超时、TLS、CORS 统一在 `Config` 与 `--xyz.*` 命名空间，模式词即命名空间（`serve --bearer=...`）。

## 安装

```bash
cargo add xyz-rust        # crate 名是 xyz-rust——crates.io 上 `xyz` 已被无关项目占用
```

导入形式 `use xyz_rust::...`。本机未发布前 crate 在仓库根（path 依赖）。要求 **Rust 1.88+**（MSRV，跟随官方 Rust SDK rmcp 3.1.4 的 `rust-version`），edition 2024。完整可运行示例见 [examples/example](examples/example/src/main.rs)（crate 名 `xyz-example`，11 条命令覆盖全部常用写法），内部机制导览见 [examples/tour](examples/tour/src/main.rs)（`xyz-tour`），与已有 clap 工程共存见 [examples/clap](examples/clap/src/main.rs)（`xyz-clap`）。编译期派生宏在附属 crate `xyz-rust-macros` 中，由 `xyz-rust` 自动引入。

## 一次定义：入参 struct 与全局契约

入参 struct 上的属性是**所有通道共享**的契约（名称、描述、默认值、必填、枚举、校验、机密性），与 Go 版的标签逐一对应：

| Go tag | Rust 属性 | 含义 | 示例 |
|---|---|---|---|
| `json:"name"` | `#[serde(rename = "name")]`（或 `#[xyz(name = "name")]`） | 线格式字段名；`#[xyz(skip)]`（≡ `json:"-"`）排除出绑定与 schema（仍可经 env/header 按 Rust 字段名注入） | `#[serde(rename = "user_name")]` |
| `desc:"..."` | `#[xyz(desc = "...")]` | 字段描述（CLI 帮助、JSON Schema description 通用） | `#[xyz(desc = "用户名")]` |
| `default:"..."` | `#[xyz(default = "...")]` | 全局默认值，按字段类型解析，可被通道覆盖 | `#[xyz(default = "18")]` |
| `required:"true"` | `#[xyz(required)]` | 必须提供 | `#[xyz(required)]` |
| `enum:"a,b"` | `#[xyz(enum = "a,b")]` | 只允许列举值（写入 schema 并在解码层强制） | `#[xyz(enum = "fast,slow")]` |
| `validate:"..."` | `#[xyz(validate = "...")]` | 校验规则（库内 validator，见下方支持集） | `#[xyz(validate = "min=2,email")]` |
| `secret:"true"` | `#[xyz(secret)]` | 敏感字段：help/日志/错误回显需打码 | `#[xyz(secret)]` |
| `cli:"..."` | `#[xyz(cli = "...")]` | CLI 绑定：`shorthand=a`、`positional`、`hidden`、`env=VAR`、`-` | `#[xyz(cli = "shorthand=a,env=TOKEN")]` |
| `http:"query"` | `#[xyz(http = "query")]` | HTTP 绑定：`query`（**未标注时默认**）/ `path` / `header` / `form` / `body` | `#[xyz(http = "header")]` |
| `httpName:"X-Key"` | `#[xyz(http_name = "X-Key")]` | HTTP 通道线上名覆盖（常用作 header 名） | `#[xyz(http_name = "X-Api-Key")]` |

派生宏同时识别极简 serde 属性子集：`#[serde(rename = "...")]`、`#[serde(rename_all = "...")]`、`#[serde(skip)]`。

`validate` 支持：`required`、`omitempty`、`min`、`max`、`len`、`gt`、`gte`、`lt`、`lte`、`oneof`、`email`（go-playground 语法的兼容子集；不支持的规则在**注册期**报错，不会静默忽略）。

类型支持：`String`、`bool`、全部整数（`i8`–`i64`、`u8`–`u64`）、`f32`/`f64`、`Vec<T>`（Go `[]T`）、`Vec<u8>`（Go `[]byte`）、`Option<T>`（Go `*T`）、嵌套 struct、`chrono::DateTime<Utc>`（Go `time.Time`）、`std::time::Duration`（Go `time.Duration`），以及经 `#[derive(XyzField)]` 的命名标量 newtype。所有接线形态都接受字符串（CLI）、JSON 形状（HTTP body）与原始 JSON（MCP），数值转换带无损检查（`3.7` 永不静默变成整数）。Rust 没有运行时反射：Go 侧 reflect 承担的事由派生宏在编译期生成。

命名标量与嵌套入参 struct：

```rust
#[derive(Clone, Copy, Debug, PartialEq, XyzField)]
struct Port(i32);

#[derive(XyzArgs)]
struct PortArgs {
    #[xyz(desc = "监听端口", default = "8080", cli = "shorthand=p")]
    port: Port,
}
```

## 通道精细配置与默认值分层

`define` 链上的 `.cli()/.http()/.mcp()` 既配命令级细节，也可通过 `fields` 映射按字段覆盖属性（两层自动合并，覆盖层零值 = 沿用属性），键为字段的 JSON 名或 Rust 名：

```rust
.cli(CliHints {
    usage:   "add <name>".into(),             // 帮助里的用法行
    aliases: vec!["ua".into(), "new".into()], // 等同子命令名
    fields: HashMap::from([
        ("age".to_string(),   CliFieldHint { shorthand: Some("a".into()), ..Default::default() }),
        ("mode".to_string(),  CliFieldHint { default: Some("fast".into()), ..Default::default() }), // 只有 CLI 有默认值
        ("token".to_string(), CliFieldHint { env_var: Some("APP_TOKEN".into()), ..Default::default() }), // env 回退
    ]),
    ..Default::default()
})
```

Hint 结构体：`CliHints { usage, aliases, hidden, fields }` 配 `CliFieldHint { shorthand, positional, hidden, skip, env_var, default }`；`HTTPHints { method, path, timeout, fields }` 配 `HTTPFieldHint { location, name, default }`；`MCPHints { annotations, fields }` 配 `MCPFieldHint { default }`。短名必须恰好一个字符（否则注册期报错）。

**同一字段的默认值优先级**（以 CLI 为例）：

```
flag 显式传值 > env 回退 > CLI 专属默认值 > 全局属性默认值（Invoke 补齐）> 零值
```

机制：各前端在调 `Entry.invoke` 前注入自己的覆盖默认（`Entry.cli_defaults()/http_defaults()/mcp_defaults()`），核心管线只有一条。MCP 的覆盖默认同时替换 `inputSchema` 里的 `default`（schema 是 MCP 的契约）。

## 三种运行形态

```
xyz-example [命令] [参数]         CLI：子命令树、短名/别名/-h/-v/--json/位置参数/env
xyz-example serve --addr :8080    HTTP：REST 路由 + /openapi.json + 同端口 /mcp
xyz-example mcp stdio|http        MCP：官方 Rust SDK，两种传输（--versions 限定协议版本）
xyz-example completion bash|zsh|fish   内置 shell 补全脚本
```
**自定义帮助块。** 纯文本自由块，多行原样输出（末尾多余换行归一为一个）；空块零影响：

```rust
// 总览开头/结尾（Config）——程序名、描述、版本、仓库地址等自己拼
.run_config(Config {
    help_before: "udf v1.0.0 — 磁盘镜像查看工具\nhttps://github.com/example/udf".into(),
    help_after: "更多示例: https://github.com/example/udf#examples".into(),
    ..Default::default()
});
// 每条命令的 -h（CliHints）：
define("extract", extract)
    .cli(CliHints { before: "extract — 解包镜像".into(), after: "仓库: https://…".into(), ..Default::default() })
```

**默认子命令**（仅 CLI）：在 `CliHints` 中标记 `default: true` 的命令成为
其父节点的默认子命令——首段参数匹配不到任何已注册命令段（且不是 flag）时，
整串参数原样转发给它：

```rust
define("extract", extract)
    .cli(CliHints { usage: "extract <image.tar>", default: true, ..Default::default() })
    .run();
// udf ./image.tar  ⟺  udf extract ./image.tar
// 每个父节点最多一个默认；flag / 显式路径 / -h / -v 均不受影响
```


**CLI**（std + serde；自带前端不引 clap——`examples/clap` 演示如何换用 clap）：注册名 `user.add` 生成两级子命令 `user add`；`-h/--help` 逐命令帮助（内联 `(default …)`/`(env …)`/`(oneof …)` 提示），`-v/--version` 输出版本（默认 `CARGO_PKG_VERSION`，可用 `set_version("v1.2.3")` 覆盖——Rust 没有 Go 的 `-ldflags -X` 等价机制）。

**HTTP**（axum）：路由即 `HTTPHints { method, path }`（`{name}` 为路径参数，绑定到 `http = "path"` 的字段）；未标注 `http:` 的字段默认从查询串绑定，JSON body 合并为入参基底；支持方法 GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS（其余在注册期报错）；状态码由错误分类映射（400/401/403/404/409/499/500/503），错误响应 `{"error":"..."}`；`GET /openapi.json` 输出由同一 `InputSchema` 生成的 OpenAPI 3 文档（含同源的响应 schema）；`GET /healthz` 探活、gzip 自动压缩。未声明路由的命令不会出现在 REST 里。

**MCP**（官方 Rust SDK rmcp）：命令即工具，`tools/list` 直接下发派生宏生成的 inputSchema 与 **outputSchema**；成功返回双份内容——`structuredContent`（裸 JSON）+ `textContent`（CLI 同款文本）；失败返回 `isError:true` + 分类消息。支持的规范版本：`2024-11-05`、`2025-03-26`、`2025-06-18`、`2025-11-25`、`2026-07-28`（最新为无握手 `server/discover` 世代），`mcp http --versions 2025-06-18,2026-07-28` 可限定；内建约束：streamable HTTP 服务 2026-07-28 需 `--stateless`（SEP-2567），且 SDK 的 streamable-HTTP 服务默认只放行 loopback `Host` 头（防 DNS rebinding）。`mcp sse` 会以清晰错误应答并退出 2——见[与 Go 版差异](#与-go-版差异)。

一条更完整的真实命令——属性、三层默认、错误分类、命名标量、`Vec<u8>`、header/env 注入都在一个定义里（取自 [examples/example/src/main.rs](examples/example/src/main.rs)）：

```rust
#[derive(XyzArgs)]
struct AddUserArgs {
    #[xyz(desc = "用户名称", required, validate = "min=2,max=32", cli = "positional", http = "path")]
    name: String,
    #[xyz(desc = "邮箱", validate = "omitempty,email")]
    email: String,
    #[xyz(desc = "年龄", default = "18", http = "query")]
    age: i32,
    #[xyz(desc = "部署模式", enum = "fast,slow", http = "query")]
    mode: String,
    #[xyz(desc = "标签", http = "query")]
    tags: Vec<String>,
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

fn add_user(_ctx: &xyz_rust::Ctx, in_: &AddUserArgs) -> errs::Result<AddUserResp> {
    if in_.name == "missing" {
        return Err(errs::new(errs::Kind::NotFound, "no such user"));
    }
    // ...
}
```

handler 形态为 `fn(&Ctx, &Args) -> Result<Resp, E>`：`E` 是任意 `std::error::Error`（错误链上已有的分类会被保留，未分类兜底 internal），`Resp` 实现 `Serialize`。`define("name", handler)` 两个类型全由推导得出——无需 Go 版的显式泛型。

### 响应呈现（无信封）

| 返回类型 | CLI | `--json` / HTTP / MCP structuredContent |
|---|---|---|
| `None` / null（或空 `Vec<T>`） | 无输出 | JSON `null`（空数组 → `[]`） |
| `string` / `bool` / 数字 | 裸值一行 | 裸 JSON 值 |
| `DateTime<Utc>` | RFC3339 | RFC3339 字符串 |
| `Vec<基础类型>` | 每行一个 | JSON 数组 |
| `struct` | 对齐 `key  value` 两列 | JSON 对象 |
| `Vec<struct>` | 对齐表格（表头+分隔线） | JSON 数组 |
| `map` | 按键值对（插入序）输出 | JSON 对象 |

struct 与 map 走同一条 `serde_json::Value` 渲染路径（`preserve_order` 保留声明序）——见[与 Go 版差异](#与-go-版差异)。

## 错误分类

handler 返回 `errs::Result<T>`（`xyz_rust::errs`）分类错误，一次分类驱动三个通道：

| Kind | HTTP | CLI 退出码 | JSON-RPC / MCP |
|---|---|---|---|
| `invalid_input`（解码/校验失败自动归类） | 400 | 2 | -32602 |
| `unauthorized` | 401 | 1 | -32010 |
| `forbidden` | 403 | 1 | -32011 |
| `not_found` | 404 | 1 | -32001 |
| `conflict` | 409 | 1 | -32009 |
| `canceled` | 499 | 1 | -32012 |
| `unavailable` | 503 | 1 | -32603 |
| 未分类（兜底 `internal`） | 500 | 1 | -32603 |

```rust
return Err(errs::new(errs::Kind::NotFound, format!("user {:?} not found", in_.name)));
// 或：  errs::errorf!(errs::Kind::NotFound, "user {} not found", in_.name)
```

## 配置：模式词与能力开关

所有派发配置集中在 `xyz_rust::Config`，零值即默认；链式用 `.configure(cfg)`，函数式用 `main_config` / `run_config`：

```rust
xyz_rust::main_config(xyz_rust::Config {
    // serve/mcp/help 是保留词，可整体改名（原词解禁，可当普通命令）
    modes: xyz_rust::ModeWords { serve: "httpd".into(), mcp: "protocol".into(), help: "assist".into() },
    // 禁用通道只移除其运行路径：mcp/serve/help/-v 壳能力永远保留
    capabilities: xyz_rust::Capabilities { no_cli: true, ..Default::default() },
    ..Default::default()
})
```

`no_cli` 禁用后不再生成用户子命令，但 `mcp stdio` / `serve` / `help` / `-v` 照常可用（总览会标注「已禁用」并隐藏命令表）；被禁用通道的 `.cli()/.http()/.mcp()` 配置照常编译与执行，只是不再被消费。没有任何已注册命令的 registry 是静默空操作（退出 0）。能力开关是运行时旋钮，与编译期 Cargo features 相互独立。

## 内置配置（--xyz.* 与 Config 字段）

库自身的配置集中在 `Config` 字段与命令行 `--xyz.*` 命名空间，优先级：**模式局部 flag > 全局 `--xyz.*` / 代码 Config > 内置默认**。`serve`/`mcp` 内部模式词就是命名空间，内置参数用裸名（`--bearer`/`--addr`/`--cors`/…），前缀 `--xyz.*` 形式可用于命令行任意位置（`--` 终止符之后除外）；模式词改名后命名空间随之迁移。

| 参数 | 代码字段 | 语义 |
|---|---|---|
| `--bearer=tok1,tok2`（或 `--bearer tok`） | `Config.bearer_tokens` | 开启 **serve REST** 与 **MCP http** 传输的 Bearer 凭据校验：`Authorization: Bearer <tok>` 命中任一 token 放行，否则 401 + `{"error":"unauthorized"}`；空 = 不校验。stdio 为本地进程不受影响（会打印提醒） |
| `--addr=:8080` | `Config.addr` | serve 与 mcp http 的默认监听地址 |
| `--log-level=debug`（或 `--xyz.log-level`） | `Config.log_level` | 库自身诊断日志（stderr，`xyz[level]:` 前缀）：`debug`/`info`/`warn`/`error`，默认 `info`。命令结果与用法错误不受影响 |
| `--timeout=45s` | `Config.timeout` | serve 的每请求超时（`tower-http` TimeoutLayer，应答 408）；0 = 不挂超时层 |
| `--tls-cert/--tls-key` | `Config.cert_file`/`key_file` | serve 同时给定则改为 TLS 监听 |
| `--cors=https://a,b`（或 `*`） | `Config.cors_origins` | serve 与 MCP http 的 CORS 白名单；预检在鉴权之前应答（浏览器预检不带凭据） |
| `--session-timeout=30m`（仅 mcp） | `mcp::Options.session_timeout` | 流式 HTTP 的空闲会话过期（SDK 的 session keep-alive） |

```bash
xyz-example serve --bearer=s3cret                    # 裸名：REST/openapi/mcp 全部要求凭据
xyz-example mcp http --addr :9000 --bearer a,b       # MCP 独立服务同款
xyz-example mcp http --xyz.bearer a,b                # 前缀形式等效
```

MCP 自带 flag：`--versions/--name/--server-version/--addr/--json-response/--stateless/--session-timeout/--bearer/--cors`。CLI 自有：`--json`、`-h`、`-v`。

**待审议清单**（保持内核最小、按需补丁）：日志输出轮转（目前 stderr 直出、无文件）、基础限流。需要哪项直接说，逐项加即可。

## 嵌入式与多注册表

单例链之外，还有显式注册表的纯函数路径（返回退出码、不结束进程，适合嵌入自己的服务、单元测试、多注册表）：

```rust
let reg = xyz_rust::Registry::new();
xyz_rust::spec::command::Command::new("user.add", add_user)
    .summary("...")
    .register(&reg)?;                                   // 显式路径
std::process::exit(xyz_rust::run(&reg, args));         // 需要 defer 清理时这样写

let srv = xyz_rust::mcp::server(&reg, xyz_rust::mcp::Options {
    versions: vec![xyz_rust::mcp::PROTOCOL_V2026_07_28.into()],
    ..Default::default()
}, ctx)?;
let router = xyz_rust::httpapi::router(&reg, ctx)?;    // 整表路由（自带 /healthz 与 /openapi.json）
let one = xyz_rust::httpapi::handler_for(entry);       // 单条命令挂任意 axum Router（entry: Arc<Entry>）
let mut app = xyz_rust::cli::App::new_with_options(&reg, xyz_rust::cli::Options::default())?;
// app.set_output(Some(out_writer), Some(err_writer)); // 重定向输出流
app.use_mw(Box::new(mw));                              // Execute 中间件：改入参、短路、包装 next()

xyz_rust::mcp::run_context(&ctx, &reg, args);
xyz_rust::cli::run_context(&ctx, &reg, args, xyz_rust::cli::Options::default());
```

派发层入口：`main_entry(&[&dyn Definable])` / `main_config(cfg)` 会结束进程，`run(&reg, args)` / `run_config(&reg, args, cfg)` 只返回退出码。

已在用 clap / axum？见[迁移指南](docs/adapters.md)，配套可运行示例 [examples/clap](examples/clap)：三种共存档位（档位 A 换前端、保留 `Entry.invoke` 脊柱；档位 B 经 `httpapi::handler_for` / `cli::App` 挂单条处理器；档位 C 借 `httpapi::middleware` / `cli::render` 积木），无需触碰核心。

## 与 Go 版差异

1. **sse 传输不存在**：官方 Rust SDK（rmcp 3.1.4）随 2026-07-28 修订移除了 HTTP+SSE 传输，`mcp sse` 可被解析但以清晰错误应答、退出码 2。可用传输：`stdio` 与流式 `http`。
2. **serde 进核心**：Rust std 无 JSON，核心（spec / registry / errors / cli / logx / 根）除 serde + serde_json（「缺位标准库」）外零第三方依赖；chrono 同为核心依赖（时间类型）。`httpapi` 基于 axum（Rust 生态事实标准 HTTP 栈，官方 rmcp 的 streamable-HTTP 示例同栈），`mcp` 基于官方 Rust SDK `rmcp`（钉定 `=3.1.4`）——唯一另一边直接第三方依赖树。`http` 与 `mcp` 共享 tokio+axum 依赖簇（内部 feature `http-stack`）。
3. **无反射**：Go 侧靠 struct tag 反射在运行时做的事，Rust 全部由派生宏编译期生成。属性词汇 `#[xyz(desc="...", name="w", required, secret, skip, validate="min=2,email", default="18", enum="a,b", cli="positional"/"shorthand=a,env=X"/"hidden"/"-", http="query|path|header|form|body", http_name="X-Key")]` 与 Go 的 tag 逐一对应，另支持 serde `rename` 回退与 `rename_all`；命名标量 newtype 用 `#[derive(XyzField)]`；结果 struct 用 `#[derive(Serialize, XyzOutput)]`（wire 名走 serde 惯例），也可不 derive——`XyzArgs` 入参 struct 自动获得 `XyzSchema`。
4. **Handler 形态**：`fn(handler(_: &Ctx, _: &Args) -> Result<Resp, E>)`，`E: std::error::Error`（错误链上的分类被保留），`R: Serialize`。`define("name", h)` 全类型推断，无 Go 的 `Define[T,R]` 显式泛型。
5. **结果渲染**：struct 与 map 同形——都先经 serde_json 序列化成 `Value`，`preserve_order` 保留声明序（Go 的 map 按键排序）；`Vec<u8>` 作为结果类型的输出 schema 是数组形（输入侧仍是 string）；`std::time::Duration` 负值不支持（Rust 语义）；oneof 对 struct 无 `%v` 形态。
6. **版本注入**：发布期调用 `set_version("v1.2.3")`——Rust 没有 `-ldflags -X` 注入，默认取 `CARGO_PKG_VERSION`。
7. **HTTP 语义**：Gzip 用 `tower-http`（任意体积响应都压缩，且完整处理 `Accept-Encoding` 的 q 值；Go 只查头）；每请求超时经 `TimeoutLayer` 应答 **408**（而非 504）；请求级取消——客户端断开不打断 handler 执行；标准头超时未配置（非零 `Config.timeout` 是唯一超时层）。
8. **MCP 差异**：`--versions` 全集与 Go 一致——`2024-11-05`、`2025-03-26`、`2025-06-18`、`2025-11-25`、`2026-07-28`（最新）——但版本钉定经 `supported_protocol_versions` 交给 SDK 协商；streamable HTTP 服务 2026-07-28 需 `--stateless`；SDK 的 streamable-HTTP 服务默认仅允许 loopback `Host` 头（rmcp 防 DNS rebinding）。
9. **能力开关在运行时仍然可用**：`Capabilities { no_cli, no_mcp, no_http }` 是 `Config` 字段，与 Cargo features 相互独立。

以上全部差异连同规范章节引用，登记在规范仓库的
[差异登记表](https://github.com/ejfkdev/xyz-spec/blob/main/deviations.md)
（`D-rust-01` … `D-rust-11`），并在 [CONFORMANCE.md](CONFORMANCE.md)
中向规范 v0.1.0 宣誓。

## 依赖原则与体积

- **核心只认 serde 家族**：核心模块除 serde、serde_json、chrono 外零第三方依赖；CLI 前端是 std + serde（无 HTTP 构建下信号只用 ctrlc）；`httpapi` 需要 axum 簇（`http-stack`：tokio、axum、axum-server、tower-http、rustls）；其上唯一的第三方树是官方 `rmcp`（钉定 `=3.1.4`，与 `mcp` 共享）。
- **Cargo features 自由裁剪**（任意组合）：

| Features | 通道 | 体积（macOS arm64，`cargo build --release` + strip，`xyz-example` 实测） |
|---|---|---|
| 默认（`cli,http,mcp`） | CLI + HTTP + MCP | 6.6M |
| `--no-default-features --features cli,http` | CLI + HTTP | 3.4M |
| `--no-default-features --features http,mcp` | HTTP + MCP | 6.5M |
| `--no-default-features --features cli,mcp` | CLI + MCP | 5.7M |
| `--no-default-features --features cli` | 仅 CLI | 0.95M |
| `--no-default-features` | 纯嵌入 | 0.81M |

```bash
cargo build --release
cargo build --release --no-default-features --features cli   # 仅 CLI
cargo build --release --no-default-features                  # 纯嵌入
strip target/release/xyz-example
```

对照：同一示例在 Go 下是 8.3M / 6.5M / 8.3M / 7.9M / 4.1M / 3.9M——Rust 更小来自没有 Go runtime 底：任何 Go 程序都要付 ≈1.1M 的运行时地板，Rust 二进制从库本体起步。裁剪掉的通道对应整块移除；调用未编译进二进制的通道会明确报错并退出 1。

## 项目结构

| 模块 | 职责 | 依赖 |
|---|---|---|
| `src/lib.rs`（根 crate `xyz_rust`） | `define` 流式链、模式分派、能力开关、内置参数（`--xyz.*`）、版本 | std + serde |
| `src/spec` | 泛型定义（派生宏生成）、解码管线、校验、JSON Schema | std + serde/serde_json + chrono |
| `src/registry` | 命令表：注册、冲突检测、进程级默认单例 | std |
| `src/errors` | 错误分类及三通道映射 | std |
| `src/cli` | CLI 前端：命令树、flag 解析、帮助、渲染、补全 | std + serde（+ctrlc） |
| `src/httpapi` | HTTP 前端：路由、入参绑定、中间件、openapi.json | axum 簇 |
| `src/logx` | 分级诊断日志（stderr，`xyz[level]:` 前缀） | std |
| `src/mcp` | MCP 前端：stdio/streamable HTTP 传输、协议版本 | rmcp |
| `src/config`、`src/builtins`、`src/dispatch`、`src/ctx`、`src/overview`、`src/version`、`src/builder` | 配置模型、`--xyz.*` 解析、根派发器、取消上下文、总览渲染、版本串、流式链 | std |
| `xyz-rust-macros/` | 派生宏 `XyzArgs` / `XyzField` / `XyzOutput` | syn + quote |
| `examples/example`、`examples/tour`、`examples/clap` | 示例、内部导览、clap 共存 | — |

## 设计原则

1. **默认路径零样板，显式路径不缺席**：链式单例是主入口；注册表参数化的纯函数是嵌入与测试的后门——两条路共用同一派发与调用管线。
2. **注册期即报错**：名字/类型/属性/路由冲突/不支持的校验规则全部在启动即表面化，不拖到运行时。
3. **配置是数据，通道是消费方**：`.cli()/.http()/.mcp()` 只是往元数据里存数据，因此 Cargo features 与能力开关任意裁剪都不影响代码编译。
4. **渲染没有信封、有自然形态**：机器读 JSON、人类读表格，同一份返回值。
5. **壳能力不可裁**：`help`/`-v`/模式词/`completion` 在任何组合下可用。
6. **模式词即命名空间**：`serve`/`mcp` 下的内置参数用裸名（`--bearer`/`--addr`/`--cors`），任意位置（`--` 之后除外）可用 `--xyz.*` 全局形式；优先级 = 模式局部 > 全局 / 代码 Config > 内置默认。库内文案不写死模式词——改名后命名空间随之迁移。
7. **取消语义贯通**：分发入口持有信号 `Ctx`，一路流入 CLI/HTTP/MCP 的 handler；HTTP 与 MCP 服务在退出前优雅排空在途请求。

## 开发

```bash
cargo test                                  # 单测覆盖库内各模块（默认 features）
cargo test --no-default-features --features cli,http   # 各裁剪组合同样通过（共 6 种组合）
cargo build --workspace                     # 全部 crate（含派生宏与示例）
cargo run -p xyz-example                    # 全家桶示例
cargo run -p xyz-tour                       # 内部机制导览
cargo run -p xyz-clap -- add bob            # clap 共存示例
```

输出契约：命令结果一律走 stdout；错误与库诊断一律走 stderr（后者带 `xyz[level]:` 前缀，级别见 `--log-level`），stdio 传输下 stdout 仅供协议帧使用。handler 收到的 `Ctx` 在收到 SIGINT/SIGTERM 时取消（HTTP/MCP 服务会先优雅关停、排空在途请求再退出）。

### 发布

```bash
cargo package                              # 发布前自检
git tag v0.1.0 && git push origin v0.1.0   # 消费者 cargo add xyz-rust（首次发布：cargo publish）
```