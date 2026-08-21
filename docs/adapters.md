# 适配指南：在已有 Rust 项目里使用 xyz-rust

xyz-rust 的核心契约与 Go 版一致：**所有前端最终都汇入
`Entry.invoke(&Ctx, & JsonMap) -> Result<Value, Error>` + 统一错误分类**
（`JsonMap = serde_json::Map<String, Value>`）。任何框架的接入都是同一
套路——你自己的解析/绑定 → `JsonMap` → `invoke` → 你想要的渲染与中间件。
兼容性不需要改动核心，只有三种「接入档位」：

| 档位 | 用什么 | 你放弃什么 | 保留什么 |
|---|---|---|---|
| A. 全替代（想用回自己的框架） | 定义层 + `invoke` 脊柱 | 我们的 CLI/HTTP 前端 | 入参定义、校验、默认值分层、错误分类、Schema |
| B. 局部替代（前端各自摘用） | `cli::App` / `httpapi::handler_for` / `mcp` 的 handler | 只有不用的那一两个前端 | 其余前端原样 |
| C. 积木复用（只借零件） | `httpapi::middleware`、`cli::render`、`InputSchema/OutputSchema` | 一切 | 只借你需要的零件 |

运行中的行为契约（所有档位通用）：

- `Entry.root` 是可读的字段树（`Vec<FieldMeta>` 挂在 `children` 上）：每个
  字段的 JSON 名、Rust 名、短名、位置参数、env、CLI/HTTP/MCP 绑定与三层
  默认值都在里面；
- `Entry.invoke` 是唯一入口：`(entry.invoke)(&ctx, &map)` —— 解码（字符串/
  JSON 都能进）、补默认、校验（含枚举）、执行 handler；
- 错误分类（`errs::classify`）在你自己框架里继续工作（映射表见主 README）；
- `#[xyz(skip)]` 的注入字段（env/header 专用）以 **Rust 字段名**为键送达
  （`f.name`）。

## 档位 A 示例：已有 clap 工程

完整可运行代码在 [`examples/clap`](../examples/clap)。要点摘录：

```rust,ignore
// 短名/位置参数/env/描述全部来自 entry.root；flag 归约成 map 后交给 invoke。
fn entry_to_clap(e: &Entry) -> clap::Command {
    // 对每个 FieldMeta：positional → 位置参数；bool → flag；Vec<T> → 多值
    // 短名 f.cli.shorthand、描述 f.description、默认值 f.cli.default/f.default
    // ...
    // cmd 构建略；执行时：
    //   map 铺底 cli_defaults() → flag 值覆盖 → env 回退 → 位置参数
    //   (e.invoke)(&ctx, &map) → cli::render(&mut out, &result)
}
```

迁移检查单（与 Go 版同构）：

1. 子命令 Use 取注册名的**最后一段**（`user.add` 拆开拼树）；
2. 位置参数 required 必须是前缀（与我们 CLI 前端同一约束）；
3. 默认值分两层：`f.default`（全局）在 invoke 里自动补，`f.cli.default`
   （CLI 专属）需要你在适配器里接管；
4. `#[xyz(skip)]` 的字段不生成 flag，env 值以 Rust 字段名为键注入；
5. handler 错误原样返回（`errs::exit_code` 可自行映射退出码）。

## 档位 B 示例：已有 axum 服务

```rust,ignore
let entry = Command::new("user.add", add_user)
    .http(HTTPHints { method: "POST".into(), path: "/users/{name}".into(), ..Default::default() })
    .register(&reg)?;

// 1. 单条命令的完整绑定处理器（query/path/header/body + 错误映射）
let app = Router::new().route("/users/{name}", post(httpapi::handler_for(entry.clone())));

// 2. 复用中间件积木：Bearer/CORS 各自独立，按你的安全策略组合
let guarded = Router::new().route("/secure/{name}", post(httpapi::handler_for(entry)))
    .layer(axum::middleware::from_fn(httpapi::middleware::bearer_mw(
        Arc::new(["s3cret".into()].into_iter().collect()),
    )));

// 3. 整表挂载（含 /openapi.json 与 /healthz）：注意剥前缀
let api = httpapi::router(&reg, Arc::new(Ctx::new()))?;
let app = Router::new().nest("/api", api);
```

### 各框架注意事项

- **axum (0.8)**：路由模板 `{name}` 语法与 `HTTPHints.path` 同构——`handler_for`
  要挂在**同模板路径**上，path 参数才能被摘出（Go 版 `r.PathValue` 同契约）。
- **嵌套路由前缀**：任何路径前缀（如 `/api`）都要在挂载外层剥掉，内部路由
  才能匹配到 `/openapi.json`/`/healthz`。
- 其他 Rust HTTP 框架（warp/tide/actix）没有 Go net/http 的统一 handler 面，
  用档位 A 打法：在你的 handler 里自己解析入参、组 `JsonMap`、调
  `(entry.invoke)(...)`，渲染与错误映射自己接（映射表见主 README）。

## 档位 C：借我们的零件

```rust,ignore
// Bearer/CORS 中间件积木来自 httpapi::middleware::bearer_mw/cors_mw，
// 包在你自己的 axum Router 外层（apply 是三者组合样例）。
let schema = &entry.input_schema;   // 直接消费（OpenAPI 文档、前端表单生成…）
let out = &entry.output_schema;
let status = errs::http_status(errs::classify(&err).unwrap_or(errs::Kind::Internal));
```

## 在 xyz 前端本身上扩展（不换框架）

- **注入输出流**：`cli::App::new_with_options(reg, cli::Options { out: Some(Box::new(w)), err_out: Some(Box::new(ew)) })`
  或 `app.set_output(...)`（嵌入大程序/测试必备）。
- **执行中间件**：`app.use_mw(ExecFunc)` 洋葱链——看到解析后的 `args`、
  改写入参、包一层计时，或**短路自绘**（不调 `next` 即接管渲染）：

```rust,ignore
app.use_mw(Box::new(move |_ctx, ec: &ExecContext, args, next| {
    if dry_run { // 自定义输出格式，跳过 Invoke+内置渲染
        writeln!(ec.out.lock().unwrap(), "would invoke {} {:?}", ec.path, args)?;
        return Ok(());
    }
    next(args)
}));
```

- **MCP 底层可扩展**：`mcp::handler::build` 返回实现了 rmcp
  `ServerHandler` 的结构，额外方法（prompts 等）可包一层代理实现后照常
  `ServiceExt::serve(...)`。