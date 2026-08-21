# xyz-rust — One definition, three interfaces

[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![MCP Protocol](https://img.shields.io/badge/MCP-2024%E2%80%932026--07--28-0764e0?style=flat)](https://modelcontextprotocol.io/specification/2026-07-28)
[![Dependencies](https://img.shields.io/badge/core-std_%2B_serde%2Fchrono-2ea44f?style=flat)](#dependency-policy--binary-size)
[![中文](https://img.shields.io/badge/中文-README.zh--CN.md-red?style=flat)](README.zh-CN.md)

The Rust port of [xyz-go](https://github.com/ejfkdev/xyz-go): **define a command once** (argument struct + validation + per-interface details) and one binary automatically speaks three interfaces — **CLI subcommands**, an **HTTP REST service** (with an OpenAPI document), and an **MCP tool server** (official Rust SDK). The library decides the running mode by itself.

```rust
use xyz_rust::errs;
use xyz_rust::{define, CliHints, HTTPHints, MCPHints, XyzArgs};

#[derive(XyzArgs)]
struct AddArgs {
    #[xyz(desc = "username", required, validate = "min=2,max=32", cli = "positional", http = "path")]
    name: String,
    #[xyz(desc = "age", default = "18")]
    age: i32,
}

fn add(_ctx: &xyz_rust::Ctx, in_: &AddArgs) -> errs::Result<String> {
    Ok(format!("{} is {}", in_.name, in_.age))
}

fn main() {
    define("user.add", add)
        .summary("Add a user")
        .cli(CliHints { usage: "add <name>".into(), ..Default::default() })
        .http(HTTPHints { method: "POST".into(), path: "/users/{name}".into(), ..Default::default() })
        .mcp(MCPHints { annotations: vec!["write".into()], ..Default::default() })
        .run(); // register + dispatch + exit — that's the whole program
}
```

```console
$ cargo run -p xyz-example -- user add bob -a 20    # CLI: subcommands + shorthand flags
id          1
name        bob
age         20
token_set   false

$ cargo run -p xyz-example -- user list             # Vec<struct> renders as an aligned table
id  name   age  token_set
--  -----  ---  ---------
1   alice  18   false
2   bob    25   false

$ curl -s -X POST localhost:8080/users/alice -d '{"age":9}'
{"id":1,"name":"alice","age":9,"token_set":false}

$ cargo run -p xyz-example -- math sum --a 1 --b 2  # bare primitive, one line
3

$ cargo run -p xyz-example -- mcp stdio            # MCP: every command becomes a tool (stdio/streamable HTTP)
```

## Features

- **One definition, zero boilerplate**: the whole `main` is a single `define(...) ... .run()` chain — no `std::process::exit`, no explicit registry, no dispatch switch.
- **One pipeline across all three interfaces**: CLI (strings), HTTP (JSON) and MCP arguments are normalized into the same `serde_json` map and flow through the same decode → defaults → validate → handler path, so behavior never drifts.
- **Per-interface fine-tuning**: shorthands, aliases, env fallbacks, binding locations — and **interface-specific default values** (two-tier layering: global attribute default → per-interface override).
- **Envelope-free responses**: primitives print bare, structs align as `key value` columns, `Vec<struct>` becomes a table, `--json` flips to JSON; HTTP answers bare JSON; MCP returns both `structuredContent` and human `textContent`.
- **One error taxonomy**: a single `errs::new(errs::Kind::NotFound, ...)` drives the CLI exit code, the HTTP status code and the MCP error code simultaneously.
- **Dependency hygiene**: the core modules (spec / registry / errors / cli / logx / root) have zero third-party dependencies apart from the serde family and chrono; the only other third-party tree is the official Rust SDK (rmcp), removable wholesale with `--no-default-features` (smallest trimmed build ≈ 0.81M).
- **Protocol versions under control**: MCP speaks the five spec revisions from 2024-11-05 to 2026-07-28; `--versions` pins the subset. Tools also carry a macro-generated `outputSchema` (OpenAPI response schemas share the same source).
- **Production-friendly**: SIGINT/SIGTERM graceful shutdown (`Ctx` flows into handlers), `/healthz` probe, gzip, CLI help with inline `(default …)`/`(env …)`/`(oneof …)` hints, and `completion bash|zsh|fish`.
- **Built-ins out of the box**: credentials (`--bearer`), default address, log level, timeout, TLS, CORS live in `Config` and the `--xyz.*` command-line namespace.

## Install

```bash
cargo add xyz-rust        # the crate name is xyz-rust — on crates.io `xyz` was already taken by an unrelated project
```

Import as `use xyz_rust::...`. The crate is not published yet; until the first release it lives at the repository root (a path dependency). Requires **Rust 1.88+** (MSRV, dictated by the official Rust SDK rmcp 3.1.4's `rust-version`) and edition 2024. A complete runnable showcase lives in [examples/example](examples/example/src/main.rs) (crate `xyz-example`, 11 commands covering the full API surface); [examples/tour](examples/tour/src/main.rs) (`xyz-tour`) walks through the internals, and [examples/clap](examples/clap/src/main.rs) (`xyz-clap`) shows coexistence with an existing clap app. The compile-time derive macros live in the companion crate `xyz-rust-macros`, pulled in automatically by `xyz-rust`.

## One definition: the argument struct

Attributes on the argument struct form the **shared contract** across every interface (wire names, descriptions, defaults, required-ness, enums, validation, secrecy). They map one-to-one onto the Go port's tags:

| Go tag | Rust attribute | Meaning | Example |
|---|---|---|---|
| `json:"name"` | `#[serde(rename = "name")]` (or `#[xyz(name = "name")]`) | Wire field name; `#[xyz(skip)]` (≡ `json:"-"`) excludes the field from binding & schema (still injectable via env/header by Rust field name) | `#[serde(rename = "user_name")]` |
| `desc:"..."` | `#[xyz(desc = "...")]` | Field description (CLI help and JSON Schema alike) | `#[xyz(desc = "username")]` |
| `default:"..."` | `#[xyz(default = "...")]` | Global default, parsed per field type, overridable per interface | `#[xyz(default = "18")]` |
| `required:"true"` | `#[xyz(required)]` | Must be provided | `#[xyz(required)]` |
| `enum:"a,b"` | `#[xyz(enum = "a,b")]` | Allowed values (enforced at decode; written into schema) | `#[xyz(enum = "fast,slow")]` |
| `validate:"..."` | `#[xyz(validate = "...")]` | Validation rules (built-in validator; see the supported set below) | `#[xyz(validate = "min=2,email")]` |
| `secret:"true"` | `#[xyz(secret)]` | Sensitive: redact in help/logs/echoes | `#[xyz(secret)]` |
| `cli:"..."` | `#[xyz(cli = "...")]` | CLI bindings: `shorthand=a`, `positional`, `hidden`, `env=VAR`, `-` | `#[xyz(cli = "shorthand=a,env=TOKEN")]` |
| `http:"query"` | `#[xyz(http = "query")]` | HTTP binding: `query` (**the default when unset**) / `path` / `header` / `form` / `body` | `#[xyz(http = "header")]` |
| `httpName:"X-Key"` | `#[xyz(http_name = "X-Key")]` | HTTP wire-name override (typically a header name) | `#[xyz(http_name = "X-Api-Key")]` |

A minimal serde subset is also honored by the derive macro: `#[serde(rename = "...")]`, `#[serde(rename_all = "...")]` and `#[serde(skip)]`.

`validate` supports: `required`, `omitempty`, `min`, `max`, `len`, `gt`, `gte`, `lt`, `lte`, `oneof`, `email` — a go-playground-compatible subset. Unsupported rules fail at **registration time**, never silently at runtime.

Type support: `String`, `bool`, all integers (`i8`–`i64`, `u8`–`u64`), `f32`/`f64`, `Vec<T>` (Go `[]T`), `Vec<u8>` (Go `[]byte`), `Option<T>` (Go `*T`), nested structs, `chrono::DateTime<Utc>` (Go `time.Time`), `std::time::Duration` (Go `time.Duration`) and named scalar newtypes via `#[derive(XyzField)]`. All wired formats accept strings (CLI), JSON shapes (HTTP body) and raw JSON (MCP) with lossless conversion checks (`3.7` never silently becomes `int(3)`). There is no runtime reflection: what reflect does on the Go side is generated at compile time by the derive macros.

Named scalar newtypes, and nested argument structs:

```rust
#[derive(Clone, Copy, Debug, PartialEq, XyzField)]
struct Port(i32);

#[derive(XyzArgs)]
struct PortArgs {
    #[xyz(desc = "listen port", default = "8080", cli = "shorthand=p")]
    port: Port,
}
```

## Per-interface configuration & default layering

`.cli()/.http()/.mcp()` on the `define` chain configure command-level details and, via the `fields` map, override attributes per field (both layers merge; a zero-value hint field means "keep the attribute"). Fields are keyed by JSON name or Rust name:

```rust
.cli(CliHints {
    usage:   "add <name>".into(),             // usage line in help
    aliases: vec!["ua".into(), "new".into()], // aliases equal subcommand names
    fields: HashMap::from([
        ("age".to_string(),   CliFieldHint { shorthand: Some("a".into()), ..Default::default() }),
        ("mode".to_string(),  CliFieldHint { default: Some("fast".into()), ..Default::default() }), // CLI-only default
        ("token".to_string(), CliFieldHint { env_var: Some("APP_TOKEN".into()), ..Default::default() }), // env fallback
    ]),
    ..Default::default()
})
```

Hint structs: `CliHints { usage, aliases, hidden, fields }` with `CliFieldHint { shorthand, positional, hidden, skip, env_var, default }`; `HTTPHints { method, path, timeout, fields }` with `HTTPFieldHint { location, name, default }`; `MCPHints { annotations, fields }` with `MCPFieldHint { default }`. A shorthand must be a single character (registration error otherwise).

**Default precedence for one field** (CLI as the example):

```
explicit flag > env fallback > interface default > global attribute default (Invoke fills it) > zero value
```

Mechanism: each frontend injects its own overrides (`Entry.cli_defaults()/http_defaults()/mcp_defaults()`) before calling `Entry.invoke`, which then applies global attribute defaults — one pipeline, drift-free. MCP's overrides also replace `default` in `inputSchema` (the schema is MCP's contract).

## Three modes

```
xyz-example [command] [args]    CLI: subcommand tree, shorthands/aliases/-h/-v/--json/positionals/env
xyz-example serve --addr :8080  HTTP: REST routes + /openapi.json + /mcp on the same port
xyz-example mcp stdio|http      MCP: official Rust SDK, two transports (--versions pins revisions)
xyz-example completion bash|zsh|fish   Built-in shell completion scripts
```

**Default subcommand** (CLI only): a command marked `default: true` in its
`CliHints` becomes the default child of its parent node — when the first
argument matches no registered segment (and is not a flag), the whole
argument list is forwarded to it unchanged:

```rust
define("extract", extract)
    .cli(CliHints { usage: "extract <image.tar>", default: true, ..Default::default() })
    .run();
// udf ./image.tar  ⟺  udf extract ./image.tar
// one default per parent node; flags/explicit paths/-h/-v are unaffected
```

**CLI** (std + serde; no clap in the shipped frontend — `examples/clap` shows how to bring your own): registry name `user.add` becomes the two-level subcommand `user add`; `-h/--help` prints per-command help (with inline `(default …)`/`(env …)`/`(oneof …)` hints), `-v/--version` prints the version (default `CARGO_PKG_VERSION`, overridable with `set_version("v1.2.3")` — Rust has no `-ldflags -X` equivalent).

**HTTP** (axum): routes come straight from `HTTPHints { method, path }` (`{name}` is a path parameter bound to a field with `http = "path"`); fields without an `http:` attribute bind from the query string by default, a JSON body merges as the argument base; supported methods are GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS (others are a registration error); the error taxonomy maps to status codes (400/401/403/404/409/499/500/503) with `{"error":"..."}` bodies; `GET /openapi.json` serves an OpenAPI 3 document from the same `InputSchema` (response schemas included); `GET /healthz` probes liveness and gzip is answered transparently. Commands without HTTP hints are not routed.

**MCP** (official Rust SDK, rmcp): commands become tools; `tools/list` serves the macro-generated `inputSchema` **and `outputSchema`**; success returns dual content — `structuredContent` (bare JSON) + `textContent` (the CLI-style rendering); failures return `isError: true` with the classified message. Supported spec revisions: `2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25`, `2026-07-28` (the newest is the handshake-free `server/discover` era); `mcp http --versions 2025-06-18,2026-07-28` pins the subset. Built-in constraints: streamable HTTP serves 2026-07-28 only with `--stateless` (SEP-2567); the SDK's streamable-HTTP server allows only loopback `Host` headers by default (its DNS-rebinding protection). `mcp sse` answers a clear error and exits 2 — see [the differences section](#differences-from-the-go-implementation).

A larger real command — attributes, three-layer defaults, error classification, named scalar, `Vec<u8>` and header/env injection all in one definition (taken from [examples/example/src/main.rs](examples/example/src/main.rs)):

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

Handlers have the shape `fn(&Ctx, &Args) -> Result<Resp, E>` where `E` is any `std::error::Error` (an existing classification is preserved along the error chain; anything unclassified falls back to `internal`) and `Resp` is `Serialize`. `define("name", handler)` fully infers both types — no explicit generic arguments needed.

### Response rendering (envelope-free)

| Return type | CLI | `--json` / HTTP / MCP structuredContent |
|---|---|---|
| `None` / null (or an empty `Vec<T>`) | nothing | JSON `null` (empty array → `[]`) |
| `string` / `bool` / numbers | bare value, one line | bare JSON value |
| `DateTime<Utc>` | RFC3339 | RFC3339 string |
| `Vec<scalars>` | one per line | JSON array |
| `struct` | aligned `key  value` columns | JSON object |
| `Vec<struct>` | aligned table (header + rule) | JSON array |
| `map` | key/value pairs in (insertion) order | JSON object |

Structs and maps render through the same `serde_json::Value` path (declaration order preserved via `preserve_order`) — see the [differences section](#differences-from-the-go-implementation).

## Error taxonomy

Handlers return `errs::Result<T>` (`xyz_rust::errs`) and one classification drives the three interfaces:

| Kind | HTTP | CLI exit | JSON-RPC / MCP |
|---|---|---|---|
| `invalid_input` (auto for decode/validation) | 400 | 2 | -32602 |
| `unauthorized` | 401 | 1 | -32010 |
| `forbidden` | 403 | 1 | -32011 |
| `not_found` | 404 | 1 | -32001 |
| `conflict` | 409 | 1 | -32009 |
| `canceled` | 499 | 1 | -32012 |
| `unavailable` | 503 | 1 | -32603 |
| unclassified (falls back to `internal`) | 500 | 1 | -32603 |

```rust
return Err(errs::new(errs::Kind::NotFound, format!("user {:?} not found", in_.name)));
// or:      errs::errorf!(errs::Kind::NotFound, "user {} not found", in_.name)
```

## Configuration: mode words & capability switches

All dispatch configuration lives in `xyz_rust::Config`; the zero value (derived `Default`) means defaults. Chain style uses `.configure(cfg)`, functional style uses `main_config` / `run_config`:

```rust
xyz_rust::main_config(xyz_rust::Config {
    // serve/mcp/help are reserved words; renaming releases the old words for use as commands
    modes: xyz_rust::ModeWords { serve: "httpd".into(), mcp: "protocol".into(), help: "assist".into() },
    // disabling a channel removes only its runtime path: mcp/serve/help/-v always survive
    capabilities: xyz_rust::Capabilities { no_cli: true, ..Default::default() },
    ..Default::default()
})
```

With `no_cli`, user subcommands disappear but `mcp stdio`, `serve`, `help` and `-v` keep working (the overview annotates "disabled" and hides the command table); the disabled channel's `.cli()/.http()/.mcp()` configuration still compiles and runs — it simply stops being consumed by a frontend that is switched off. A registry with zero registered commands is a silent no-op (exit 0). Capability switches are runtime knobs, independent of the compile-time Cargo features.

## Built-in configuration (`--xyz.*` and `Config` fields)

The library's own settings live in `Config` fields and the `--xyz.*` command-line namespace; precedence: **mode-local flag > global `--xyz.*` / code Config > library defaults**. Inside `serve`/`mcp` the mode word _is_ the namespace, so built-ins use bare names (`--bearer`, `--addr`, `--cors`, …), and the prefixed `--xyz.*` forms work anywhere before the `--` terminator. Renaming the mode words migrates the namespace with them.

| Parameter | Code field | Meaning |
|---|---|---|
| `--bearer=tok1,tok2` (or `--bearer tok`) | `Config.bearer_tokens` | Bearer verification for **serve REST** and **MCP http**: `Authorization: Bearer <tok>` must hit one of the tokens, else 401 + `{"error":"unauthorized"}`; empty = no auth. stdio is a local process and unaffected (a note is logged) |
| `--addr=:8080` | `Config.addr` | Default listen address for serve and mcp http |
| `--log-level=debug` (or `--xyz.log-level`) | `Config.log_level` | Library diagnostics to stderr (`xyz[level]:` prefix): `debug`/`info`/`warn`/`error`, default `info`. Command results and usage errors are unaffected |
| `--timeout=45s` | `Config.timeout` | serve per-request timeout (a `tower-http` TimeoutLayer answering 408); 0 = no timeout layer |
| `--tls-cert/--tls-key` | `Config.cert_file`/`key_file` | serve switches to TLS when both are given |
| `--cors=https://a,b` (or `*`) | `Config.cors_origins` | CORS allowlist for serve and MCP http; OPTIONS preflights answer before auth (browser preflights carry no credentials) |
| `--session-timeout=30m` (mcp only) | `mcp::Options.session_timeout` | Idle-session expiry for streamable HTTP (the SDK's session keep-alive) |

```bash
xyz-example serve --bearer=s3cret                    # REST/openapi/mcp all require credentials
xyz-example mcp http --addr :9000 --bearer a,b       # standalone MCP, same scheme
xyz-example mcp http --xyz.bearer a,b                # prefixed form is equivalent
```

MCP's own flags: `--versions/--name/--server-version/--addr/--json-response/--stateless/--session-timeout/--bearer/--cors`. CLI owns `--json`, `-h`, `-v`.

Waiting list (kept out to stay minimal — each lands in one iteration when asked): log file rotation, basic rate limiting.

## Embedding & multiple registries

Besides the singleton chain, the parameterized pure functions (which return the exit code without exiting) serve embedding, tests and multi-registry setups:

```rust
let reg = xyz_rust::Registry::new();
xyz_rust::spec::command::Command::new("user.add", add_user)
    .summary("...")
    .register(&reg)?;                                   // explicit path
std::process::exit(xyz_rust::run(&reg, args));         // for deferred cleanup in main

let srv = xyz_rust::mcp::server(&reg, xyz_rust::mcp::Options {
    versions: vec![xyz_rust::mcp::PROTOCOL_V2026_07_28.into()],
    ..Default::default()
}, ctx)?;
let router = xyz_rust::httpapi::router(&reg, ctx)?;    // mount all HTTP routes (healthz & openapi included)
let one = xyz_rust::httpapi::handler_for(entry);       // mount one command on any axum Router (entry: Arc<Entry>)
let mut app = xyz_rust::cli::App::new_with_options(&reg, xyz_rust::cli::Options::default())?;
// app.set_output(Some(out_writer), Some(err_writer)); // redirect output streams
app.use_mw(Box::new(mw));                              // Execute middleware: rewrite args, short-circuit, wrap next()

xyz_rust::mcp::run_context(&ctx, &reg, args);
xyz_rust::cli::run_context(&ctx, &reg, args, xyz_rust::cli::Options::default());
```

The dispatch-level entry points are `main_entry(&[&dyn Definable])` / `main_config(cfg)` (which exit) and `run(&reg, args)` / `run_config(&reg, args, cfg)` (which return the exit code).

Already on clap / axum? See the [migration guide](docs/adapters.md) with a runnable example in [`examples/clap`](examples/clap) — three coexistence levels (tier A: replace the frontend and keep the `Entry.invoke` spine; tier B: mount per-command handlers via `httpapi::handler_for` / `cli::App`; tier C: reuse parts like `httpapi::middleware` or `cli::render`), all without touching the core.

## Differences from the Go implementation

1. **No SSE transport.** The official Rust SDK (rmcp 3.1.4) removed the HTTP+SSE transport with the 2026-07-28 revision, so `mcp sse` is parsed but answered with a clear error and exit code 2. Available transports: `stdio` and streamable `http`.
2. **serde enters the core.** Rust's std has no JSON, so the core (spec / registry / errors / cli / logx / root) depends on serde + serde_json — the "missing standard library" — and chrono (time types), and nothing else. `httpapi` sits on axum (the de-facto standard HTTP stack in Rust, the same stack the official rmcp streamable-HTTP examples use), and `mcp` on the official Rust SDK `rmcp` pinned to `=3.1.4` — the only other direct third-party dependency tree. `http` and `mcp` share the tokio+axum cluster behind the internal `http-stack` feature.
3. **No reflection.** What Go derives at runtime from struct tags is generated at compile time by the derive macros. The attribute vocabulary is `#[xyz(desc="...", name="w", required, secret, skip, validate="min=2,email", default="18", enum="a,b", cli="positional"/"shorthand=a,env=X"/"hidden"/"-", http="query|path|header|form|body", http_name="X-Key")]` — one-to-one with the Go tags, plus the serde `rename` fallback and `rename_all` support. Named scalar newtypes derive `XyzField`; result structs derive `Serialize` + `XyzOutput` with wire names following serde conventions, or no derive at all — a `XyzArgs` input struct automatically provides `XyzSchema`.
4. **Handler shape.** `fn(handler(_: &Ctx, _: &Args) -> Result<Resp, E>)` with `E: std::error::Error` (classification on the error chain is preserved) and `R: Serialize`. `define("name", h)` is fully inferred — no Go-style explicit `Define[T, R]` generics.
5. **Result rendering.** Structs and maps are the same shape: both become a `serde_json::Value` after serialization, and `preserve_order` keeps declaration order (Go sorts map keys); a `Vec<u8>` result type gets an array-shaped output schema (its input side is still `string`); `std::time::Duration` has no negative-value support (Rust semantics); `oneof` has no `%v` form for structs.
6. **Version injection.** Call `set_version("v1.2.3")` at release time — Rust has no `-ldflags -X`; the default is `CARGO_PKG_VERSION`.
7. **HTTP semantics.** Gzip via `tower-http` (compresses any response size and handles `Accept-Encoding` q-values; the Go port only checks the header); per-request timeout via `TimeoutLayer` answering **408** (not 504); request-level cancellation: a client disconnect does not interrupt the running handler; the standard header timeout is not configured (a non-zero `Config.timeout` is the only timeout layer).
8. **MCP differences.** `--versions` accepts the same full set as Go — `2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25`, `2026-07-28` (newest) — but version pinning is handed to the SDK's negotiation via `supported_protocol_versions`; streamable HTTP serves 2026-07-28 only with `--stateless`; and the SDK's streamable-HTTP server allows only loopback `Host` headers by default (rmcp's DNS-rebinding protection).
9. **Capability switches stay available at runtime**: `Capabilities { no_cli, no_mcp, no_http }` are `Config` fields, independent of the Cargo features.

All of the above are filed, with spec-section references, in the spec repository's [deviations register](https://github.com/ejfkdev/xyz-spec/blob/main/deviations.md) (`D-rust-01` … `D-rust-11`) and attested in [CONFORMANCE.md](CONFORMANCE.md).

## Dependency policy & binary size

- **Serde-family-only core**: the core modules have zero third-party dependencies beyond serde, serde_json and chrono; the CLI frontend is std + serde (ctrlc only for signals in non-HTTP builds); `httpapi` needs the axum cluster (`http-stack`: tokio, axum, axum-server, tower-http, rustls); the only third-party tree on top is the official `rmcp` (pinned `=3.1.4`, shared by `mcp`).
- **Trim any channel via Cargo features** (any combination):

| Features | Channels | Size (macOS arm64, `cargo build --release` + strip, `xyz-example`) |
|---|---|---|
| default (`cli,http,mcp`) | CLI + HTTP + MCP | 6.6M |
| `--no-default-features --features cli,http` | CLI + HTTP | 3.4M |
| `--no-default-features --features http,mcp` | HTTP + MCP | 6.5M |
| `--no-default-features --features cli,mcp` | CLI + MCP | 5.7M |
| `--no-default-features --features cli` | CLI only | 0.95M |
| `--no-default-features` | embedding only | 0.81M |

```bash
cargo build --release
cargo build --release --no-default-features --features cli   # CLI only
cargo build --release --no-default-features                  # embedding only
strip target/release/xyz-example
```

For reference, the same showcase in Go measures 8.3M / 6.5M / 8.3M / 7.9M / 4.1M / 3.9M — Rust comes out smaller because there is no Go runtime floor: every Go binary pays ≈1.1M just for the runtime, while a Rust binary starts from the library itself. The trimmed channel disappears as a block; invoking a channel that was not compiled in answers a clear error and exits 1.

## Module layout

| Module | Responsibility | Dependencies |
|---|---|---|
| `src/lib.rs` (root crate `xyz_rust`) | fluent `define` chain, mode dispatch, capability switches, built-in parameters (`--xyz.*`), version | std + serde |
| `src/spec` | generic definition (derive-macro generated), decode pipeline, validation, JSON Schema | std + serde/serde_json + chrono |
| `src/registry` | command table: registration, conflict checks, the process-level singleton | std |
| `src/errors` | error taxonomy and three-interface mappings | std |
| `src/cli` | CLI frontend: command tree, flag parsing, help, rendering, completion | std + serde (+ctrlc) |
| `src/httpapi` | HTTP frontend: routing, binding, middleware, openapi.json | axum cluster |
| `src/logx` | leveled diagnostics to stderr (`xyz[level]:` prefix) | std |
| `src/mcp` | MCP frontend: stdio/streamable HTTP transports, protocol versions | rmcp |
| `src/config`, `src/builtins`, `src/dispatch`, `src/ctx`, `src/overview`, `src/version`, `src/builder` | configuration model, `--xyz.*` parsing, root dispatcher, cancellation context, overview rendering, version string, fluent chain | std |
| `xyz-rust-macros/` | derive macros `XyzArgs` / `XyzField` / `XyzOutput` | syn + quote |
| `examples/example`, `examples/tour`, `examples/clap` | showcase, internal tour, clap coexistence | — |

## Design principles

1. **Zero boilerplate by default, explicit paths always available**: the singleton chain is the main entry; registry-parameterized pure functions are the backdoor for embedding and tests — both share one dispatch and invoke pipeline.
2. **Fail at registration**: bad names/types/attributes/route conflicts/unsupported validation rules surface at startup, never at runtime.
3. **Configuration is data; frontends are consumers**: `.cli()/.http()/.mcp()` only store metadata, so Cargo features and capability switches never break compilation.
4. **Envelope-free, natural forms**: machines read JSON, humans read tables, from one return value.
5. **The shell is uncuttable**: `help`/`-v`/mode words/`completion` work in every combination.
6. **The mode word is the namespace**: built-ins in `serve`/`mcp` use bare names; `--xyz.*` works globally; library messages never hardcode mode words — renaming migrates everything.
7. **Cancellation flows everywhere**: the dispatcher owns a signal `Ctx` that reaches CLI/HTTP/MCP handlers; HTTP and MCP servers drain in-flight requests (graceful window) before exiting.

## Development

```bash
cargo test                                  # unit/integration tests across the library (default features)
cargo test --no-default-features --features cli,http   # every trimmed feature combination also passes (6 combos)
cargo build --workspace                     # all crates incl. macros + examples
cargo run -p xyz-example                    # full showcase
cargo run -p xyz-tour                       # internal-walkthrough tour
cargo run -p xyz-clap -- add bob            # clap coexistence example
```

Output contract: command results go to stdout; errors and diagnostics go to stderr (diagnostics carry the `xyz[level]:` prefix, level via `--log-level`); under the stdio transport stdout is reserved for protocol frames. The `Ctx` passed to handlers is canceled on SIGINT/SIGTERM (the HTTP/MCP servers drain in-flight requests first).

### Release

```bash
cargo package                              # sanity check before publishing
git tag v0.1.0 && git push origin v0.1.0   # consumers: cargo add xyz-rust (first publish: cargo publish)
```

> Also available: [中文文档](README.zh-CN.md)