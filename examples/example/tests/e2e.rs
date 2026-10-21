// 端到端测试：驱动真实示例二进制（`xyz-example`）跨四个通道验证行为。
//
// 覆盖面（单元测试难以触及的部分）：CLI 的格式轴/别名/环境注入/退出码/
// 帮助/内容块落盘/管道早关；HTTP 的绑定与严格 query/OpenAPI/头/gzip/CORS/
// Bearer/TLS/内容块/联合；MCP stdio 的握手/工具表/结构化输出门控/版本门控/
// 错误/_meta/内容块/联合；MCP streamable HTTP 的 legacy 会话、stateless
// 2026-07-28 与 --json-response。
//
// 只依赖 std + serde_json；HTTP 客户端是手写的（服务端全部走 Connection:
// close，SSE 读超时兜底），避免为测试引入第三方 HTTP 客户端。

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_xyz-example")
}

// ---------------------------------------------------------------- CLI 驱动

#[derive(Debug)]
struct Out {
    code: i32,
    out: String,
    err: String,
}

fn run(args: &[&str]) -> Out {
    run_env(args, &[])
}

fn run_env(args: &[&str], envs: &[(&str, &str)]) -> Out {
    let mut c = Command::new(bin());
    c.args(args);
    for (k, v) in envs {
        c.env(k, v);
    }
    let o = c.output().expect("spawn xyz-example");
    Out {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

// ---------------------------------------------------------------- 服务器

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe");
    l.local_addr().expect("local addr").port()
}

/// 启动 `serve`/`http`/`mcp http` 子进程并等待端口可连。
fn start(mode: &str, extra: &[&str]) -> Server {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut c = Command::new(bin());
    c.args([mode, "--addr", &addr]).args(extra);
    c.stdout(Stdio::null());
    c.stderr(Stdio::piped());
    let mut child = c.spawn().expect("spawn server");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(st) = child.try_wait().expect("try_wait") {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            panic!("server ({mode}) exited early: {st}: {err}");
        }
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Server { child, port };
        }
        if Instant::now() > deadline {
            panic!("server ({mode}) did not start listening");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------- HTTP 客户端

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(self.body.trim()).unwrap_or(Value::Null)
    }
    /// SSE 里的 JSON-RPC 负载（跳过空 data 与 keep-alive 帧）。
    fn sse_payloads(&self) -> Vec<Value> {
        self.body
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .collect()
    }
}

fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Resp {
    let mut req =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || matches!(method, "POST" | "PUT" | "PATCH") {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    req.push_str(body);

    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(req.as_bytes()).expect("write request");
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match s.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            // SSE 流不会随 Connection: close 立刻结束：读超时即视为本次响应结束。
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Resp {
        status,
        headers,
        body: body.to_string(),
    }
}

fn get(port: u16, path: &str) -> Resp {
    http(port, "GET", path, &[], "")
}

fn get_h(port: u16, path: &str, headers: &[(&str, &str)]) -> Resp {
    http(port, "GET", path, headers, "")
}

fn post_json(port: u16, path: &str, body: Value) -> Resp {
    http(
        port,
        "POST",
        path,
        &[("Content-Type", "application/json")],
        &body.to_string(),
    )
}

// ---------------------------------------------------------------- MCP 驱动

fn init(id: i64, ver: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
        "protocolVersion": ver, "capabilities": {}, "clientInfo": {"name":"e2e","version":"0"}}})
}

fn initialized() -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
}

fn by_id(msgs: &[Value], id: i64) -> &Value {
    msgs.iter()
        .find(|m| m.get("id").and_then(|v| v.as_i64()) == Some(id))
        .unwrap_or_else(|| panic!("no response with id {id} in {msgs:?}"))
}

/// stdio 一轮：写入所有请求，关闭 stdin（服务端 EOF 退出），读回所有响应。
fn mcp_stdio(args: &[&str], reqs: &[Value]) -> Vec<Value> {
    let mut c = Command::new(bin());
    c.args(["mcp", "stdio"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = c.spawn().expect("spawn mcp stdio");
    {
        let mut si = child.stdin.take().expect("stdin");
        for r in reqs {
            writeln!(si, "{r}").expect("write request");
        }
    }
    let out = child.wait_with_output().expect("wait mcp stdio");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn tool_call(id: i64, name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})
}

// ================================================================ CLI

#[test]
fn cli_formats_aliases_and_env() {
    // 捕获的 stdout 是管道 → --format auto 解析为 jsonl（spec §10.7）。
    let auto = run(&["user", "list"]);
    assert_eq!(auto.code, 0, "{}", auto.err);
    assert_eq!(
        auto.out.lines().count(),
        2,
        "auto(pipe) should be jsonl: {}",
        auto.out
    );

    // 显式 text：对齐表格（表头 + 分隔线 + 两行数据）。
    let text = run(&["user", "list", "--format", "text"]);
    assert_eq!(text.code, 0, "{}", text.err);
    assert!(
        text.out.contains("alice") && text.out.contains("bob"),
        "{}",
        text.out
    );
    assert!(
        text.out.contains("---"),
        "table should have a rule: {}",
        text.out
    );

    // --format json / --json 别名。
    for args in [
        vec!["user", "list", "--format", "json"],
        vec!["user", "list", "--json"],
    ] {
        let o = run(&args);
        assert_eq!(o.code, 0, "{}", o.err);
        let v: Value = serde_json::from_str(o.out.trim()).expect("json output");
        assert_eq!(v.as_array().map(|a| a.len()), Some(2), "{v}");
    }

    // jsonl：一行一个对象。
    let jl = run(&["user", "list", "--format", "jsonl"]);
    assert_eq!(jl.code, 0, "{}", jl.err);
    let rows: Vec<Value> = jl
        .out
        .lines()
        .map(|l| serde_json::from_str(l).expect("jsonl line"))
        .collect();
    assert_eq!(rows.len(), 2);

    // markdown：结构体走 Field/Value 表。
    let md = run(&["user", "add", "alice", "--format", "markdown"]);
    assert_eq!(md.code, 0, "{}", md.err);
    assert!(md.out.contains("| Field | Value |"), "{}", md.out);

    // 别名 + 位置参数。
    let alias = run(&["user", "ua", "carol", "--format", "json"]);
    assert_eq!(alias.code, 0, "{}", alias.err);
    let v: Value = serde_json::from_str(alias.out.trim()).unwrap();
    assert_eq!(v["name"], "carol");
    assert_eq!(v["id"], 1, "fresh process resets the id counter");

    // env 注入（skip/secret 字段不进 schema）。
    let env = run_env(
        &["user", "add", "alice", "--format", "json"],
        &[("APP_TOKEN", "t0ken")],
    );
    assert_eq!(env.code, 0, "{}", env.err);
    let v: Value = serde_json::from_str(env.out.trim()).unwrap();
    assert_eq!(v["token_set"], true);
    // secret 不出现在帮助里。
    let help = run(&["user", "add", "-h"]);
    assert!(!help.out.contains("APP_TOKEN") && !help.out.contains("t0ken"));
}

#[test]
fn cli_exit_codes_and_errors() {
    // 校验失败 → invalid_input → exit 2。
    let bad = run(&["user", "add", "a"]);
    assert_eq!(bad.code, 2, "{:?}", bad);
    assert!(bad.err.contains("name"), "{}", bad.err);

    // 未知 flag → exit 2 + usage 提示。
    let unknown = run(&["user", "list", "--bogus"]);
    assert_eq!(unknown.code, 2);
    assert!(
        unknown.err.contains("unknown") || unknown.err.contains("bogus"),
        "{}",
        unknown.err
    );

    // 业务错误（not_found）→ exit 1。
    let nf = run(&["user", "rm", "bob"]);
    assert_eq!(nf.code, 1, "{}", nf.err);
    assert!(nf.err.contains("not found"), "{}", nf.err);
    let ok = run(&["user", "rm", "alice"]);
    assert_eq!(ok.code, 0);
    assert!(ok.out.contains("removed"));

    // 联合字段在 CLI 被跳过：缺必填时是干净的分类错误，不是 panic。
    let union = run(&["pick", "by"]);
    assert_eq!(union.code, 2);
    assert!(union.err.contains("sel"), "{}", union.err);
    assert!(!union.err.contains("panicked"), "{}", union.err);

    // --format json 下错误体走 stderr 的 §8.6 形态。
    let machine = run(&["user", "add", "a", "--format", "json"]);
    assert_eq!(machine.code, 2);
    let v: Value = serde_json::from_str(machine.err.trim()).expect("machine error body");
    assert_eq!(v["kind"], "invalid_input", "{v}");
}

#[test]
fn cli_help_modes_and_version() {
    let overview = run(&["--help"]);
    assert_eq!(overview.code, 0);
    for needle in ["serve", "http", "mcp", "help", "blocks.show", "pick.by"] {
        assert!(
            overview.out.contains(needle),
            "overview missing {needle}: {}",
            overview.out
        );
    }

    let cmd_help = run(&["help", "user.add"]);
    assert_eq!(cmd_help.code, 0);
    assert!(cmd_help.out.contains("Usage"), "{}", cmd_help.out);

    let mode_help = run(&["help", "serve"]);
    assert_eq!(mode_help.code, 0);
    assert!(mode_help.out.contains("REST"), "{}", mode_help.out);

    // 模式 -h 只打帮助、不启动服务器（必须很快返回）。
    let t0 = Instant::now();
    let mcp_help = run(&["mcp", "-h"]);
    assert_eq!(mcp_help.code, 0);
    assert!(mcp_help.out.contains("stdio"), "{}", mcp_help.out);
    assert!(t0.elapsed() < Duration::from_secs(5));

    let version = run(&["-v"]);
    assert_eq!(version.code, 0);
    assert!(version.out.contains("xyz-example"), "{}", version.out);
}

#[test]
fn cli_blocks_spill_and_broken_pipe() {
    let o = run(&["blocks", "show", "--format", "text"]);
    assert_eq!(o.code, 0, "{}", o.err);
    let mut lines = o.out.lines();
    assert_eq!(lines.next(), Some("hello"), "text block first: {}", o.out);
    let path = lines.next().expect("image spill path");
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    assert_eq!(
        &bytes[..8],
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        "spilled file must hold the decoded PNG magic"
    );
    let _ = std::fs::remove_file(path);

    // 管道早关（| head -1）静默退出 141（SIGPIPE 惯例）。
    let script = format!(
        "{} sys seq --n 100000 --format text | head -1; exit ${{PIPESTATUS[0]}}",
        bin()
    );
    let st = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("bash");
    assert_eq!(st.code(), Some(141));
}

// ================================================================ HTTP

#[test]
fn http_binding_strict_query_and_errors() {
    let srv = start("serve", &[]);
    // 默认注册 GET+POST；path/query/数组参数。
    let r = get(srv.port, "/users/alice?age=20&mode=fast&tags=a&tags=b");
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["name"], "alice");
    assert_eq!(v["age"], 20);

    // 未知 query → 400 invalid_input，消息含参数名与允许集。
    let bad = get(srv.port, "/users/alice?bogus=1");
    assert_eq!(bad.status, 400, "{}", bad.body);
    let v = bad.json();
    assert_eq!(v["kind"], "invalid_input", "{v}");
    let msg = v["error"].as_str().unwrap_or_default();
    assert!(msg.contains("bogus") && msg.contains("age"), "{msg}");

    // 校验（email）失败 → 400。
    let invalid = get(srv.port, "/users/alice?email=nope");
    assert_eq!(invalid.status, 400, "{}", invalid.body);

    // POST JSON body 合并；bool/数组字段。
    let p = post_json(
        srv.port,
        "/users/bob",
        json!({"age": 30, "verbose": true, "tags": ["x"]}),
    );
    assert_eq!(p.status, 200, "{}", p.body);
    assert_eq!(p.json()["age"], 30);

    // header 注入字段（skip + http_name）。
    let h = get_h(srv.port, "/headers", &[("X-Api-Key", "k123")]);
    assert_eq!(h.status, 200, "{}", h.body);
    assert!(h.body.contains("4 bytes"), "{}", h.body);
    let no_key = get(srv.port, "/headers");
    assert_eq!(no_key.status, 401, "{}", no_key.body);

    // 业务错误 → 404 + 分类 kind；未知路由 → 404。
    let nf = get(srv.port, "/users/missing");
    assert_eq!(nf.status, 404, "{}", nf.body);
    assert_eq!(nf.json()["kind"], "not_found");
    let miss = get(srv.port, "/nope");
    assert_eq!(miss.status, 404);
    assert!(miss.json()["error"].is_string());

    // healthz 字节精确。
    let hz = get(srv.port, "/healthz");
    assert_eq!(hz.status, 200);
    assert_eq!(hz.body, "{\"status\":\"ok\"}\n");
}

#[test]
fn http_openapi_headers_gzip_cors_bearer() {
    let srv = start("serve", &[]);
    let doc = get(srv.port, "/openapi.json").json();
    assert_eq!(doc["openapi"], "3.0.3");
    assert_eq!(doc["info"]["title"], "xyz-example");
    let users = &doc["paths"]["/users/{name}"];
    // 空 method → GET+POST 两条 operation。
    assert!(
        users.get("get").is_some() && users.get("post").is_some(),
        "{users}"
    );
    let params = users["get"]["parameters"].as_array().expect("parameters");
    let names: Vec<&str> = params.iter().filter_map(|p| p["name"].as_str()).collect();
    for n in ["name", "email", "age", "mode", "tags", "limit", "verbose"] {
        assert!(names.contains(&n), "missing parameter {n}: {names:?}");
    }
    let name_p = params.iter().find(|p| p["name"] == "name").unwrap();
    assert_eq!(name_p["in"], "path");
    assert_eq!(name_p["required"], true);
    assert!(users["post"].get("requestBody").is_some());
    // 响应 schema 与 outputSchema 同源（object）。
    assert_eq!(
        users["get"]["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        "object"
    );

    // 服务器上下文头。
    let r = get(srv.port, "/healthz");
    assert_eq!(r.header("x-app-name"), Some("xyz-example"));
    assert!(r.header("x-xyz-version").is_some_and(|v| !v.is_empty()));

    // gzip。
    let gz = get_h(srv.port, "/healthz", &[("Accept-Encoding", "gzip")]);
    assert_eq!(gz.status, 200);
    assert_eq!(gz.header("content-encoding"), Some("gzip"));

    // 内容块：HTTP 信封原样。
    let blocks = get(srv.port, "/blocks").json();
    assert_eq!(blocks["content"][0]["type"], "text");
    assert_eq!(blocks["content"][1]["type"], "image");
    assert_eq!(blocks["content"][1]["mimeType"], "image/png");
    assert_eq!(blocks["content"][1]["data"], "iVBORw0KGgo=");

    // 联合：Good/Bad。
    let ok = post_json(srv.port, "/pick", json!({"sel": {"kind": "ById", "id": 7}}));
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert!(ok.body.contains("id=7"), "{}", ok.body);
    let bad = post_json(srv.port, "/pick", json!({"sel": {"kind": "Nope"}}));
    assert_eq!(bad.status, 400, "{}", bad.body);
    assert_eq!(bad.json()["kind"], "invalid_input");

    // 放宽开关。
    let lenient = start("serve", &["--xyz.allow-unknown-query"]);
    let r = get(lenient.port, "/users/alice?bogus=1");
    assert_eq!(r.status, 200, "{}", r.body);

    // 关闭自动头。
    let bare = start("serve", &["--xyz.no-server-headers"]);
    let r = get(bare.port, "/healthz");
    assert!(r.header("x-app-name").is_none());
    assert!(r.header("x-xyz-version").is_none());

    // CORS 预检。
    let cors = start("serve", &["--cors", "http://example.test"]);
    let pre = http(
        cors.port,
        "OPTIONS",
        "/users/alice",
        &[
            ("Origin", "http://example.test"),
            ("Access-Control-Request-Method", "GET"),
        ],
        "",
    );
    assert!(pre.status == 200 || pre.status == 204, "{}", pre.status);
    assert_eq!(
        pre.header("access-control-allow-origin"),
        Some("http://example.test")
    );

    // Bearer 鉴权；预检在鉴权之前应答（浏览器不带 Authorization）。
    let auth = start("serve", &["--bearer", "s3cret"]);
    assert_eq!(get(auth.port, "/healthz").status, 401);
    let ok = get_h(auth.port, "/healthz", &[("Authorization", "Bearer s3cret")]);
    assert_eq!(ok.status, 200);
    // 预检在鉴权之前应答：同时配置 --cors 与 --bearer 时，浏览器式预检
    // （不带 Authorization）仍应 204 且带 ACAO。
    let both = start(
        "serve",
        &["--bearer", "s3cret", "--cors", "http://example.test"],
    );
    let pre = http(
        both.port,
        "OPTIONS",
        "/healthz",
        &[
            ("Origin", "http://example.test"),
            ("Access-Control-Request-Method", "GET"),
        ],
        "",
    );
    assert_eq!(pre.status, 204, "preflight must bypass bearer");
    assert_eq!(
        pre.header("access-control-allow-origin"),
        Some("http://example.test")
    );
}

#[test]
fn http_tls_serves_requests() {
    // openssl 缺失时跳过（CI 与 macOS 默认都有）。
    if Command::new("openssl").arg("version").output().is_err() {
        eprintln!("skip: openssl not available");
        return;
    }
    let dir = std::env::temp_dir().join(format!("xyz-e2e-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    let st = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
        ])
        .arg("-keyout")
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("openssl req");
    assert!(st.success(), "openssl req failed");
    let srv = start(
        "serve",
        &[
            "--tls-cert",
            cert.to_str().unwrap(),
            "--tls-key",
            key.to_str().unwrap(),
        ],
    );
    // openssl s_client 不做证书校验，直接把 HTTP 请求写进 TLS 通道。
    let mut child = Command::new("openssl")
        .args([
            "s_client",
            "-quiet",
            "-connect",
            &format!("127.0.0.1:{}", srv.port),
            "-servername",
            "localhost",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("openssl s_client");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let out = child.wait_with_output().expect("s_client wait");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("200 OK"), "TLS response: {text}");
    assert!(text.contains("{\"status\":\"ok\"}"), "TLS body: {text}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ================================================================ MCP stdio

#[test]
fn mcp_stdio_capabilities_tools_and_gating() {
    let msgs = mcp_stdio(
        &[],
        &[
            init(1, "2025-11-25"),
            initialized(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        ],
    );
    let hello = by_id(&msgs, 1);
    assert_eq!(hello["result"]["protocolVersion"], "2025-11-25");
    assert!(
        hello["result"]["capabilities"]["tools"].is_object(),
        "tools capability must be declared: {hello}"
    );
    assert_eq!(hello["result"]["serverInfo"]["name"], "xyz-example");

    let tools = by_id(&msgs, 2)["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(tools.len(), 14, "one tool per command");
    let find = |n: &str| {
        tools
            .iter()
            .find(|t| t["name"] == n)
            .unwrap_or_else(|| panic!("tool {n} missing"))
    };
    // object 结果带 outputSchema；数组/标量结果不带（§12.3）。
    assert_eq!(find("user.add")["outputSchema"]["type"], "object");
    // 块结果没有单一 JSON Schema（信封形状由前端识别），故不挂 outputSchema。
    assert!(
        find("blocks.show").get("outputSchema").is_none(),
        "block results carry no outputSchema"
    );
    assert!(
        find("math.sum").get("outputSchema").is_none(),
        "scalar result must not advertise a schema"
    );
    assert!(
        find("search.query").get("outputSchema").is_none(),
        "array result must not advertise a schema"
    );
    assert!(
        find("user.list").get("outputSchema").is_none(),
        "Vec<struct> is still an array"
    );
    assert!(
        find("pick.by").get("outputSchema").is_none(),
        "string result must not advertise a schema"
    );
    // 联合的 oneOf + const 判别。
    let sel = &find("pick.by")["inputSchema"]["properties"]["sel"];
    let oneof = sel["oneOf"].as_array().expect("union oneOf");
    assert_eq!(oneof.len(), 2, "{sel}");
    let tags: Vec<&str> = oneof
        .iter()
        .filter_map(|v| v["properties"]["kind"]["const"].as_str())
        .collect();
    assert!(
        tags.contains(&"ById") && tags.contains(&"ByName"),
        "{sel:?}"
    );
    // inputSchema 默认值与 MCPFieldHint 覆写（search.query k=15）。
    assert_eq!(
        find("search.query")["inputSchema"]["properties"]["k"]["default"],
        15
    );

    // 调用 + _meta.xyz。
    let msgs = mcp_stdio(
        &[],
        &[
            init(1, "2025-11-25"),
            initialized(),
            tool_call(2, "math.sum", json!({"a": 1, "b": 2})),
        ],
    );
    let r = &by_id(&msgs, 2)["result"];
    // 标量结果：只有 textContent（§12.5 的 object-only 规则）。
    assert!(r.get("structuredContent").is_none(), "{r}");
    assert_eq!(r["content"][0]["text"], "3", "{r}");
    let meta = &r["_meta"]["xyz"];
    assert_eq!(meta["command"], "math.sum");
    assert_eq!(meta["app_name"], "xyz-example");
    assert_eq!(meta["sdk_version"], xyz_rust::version::SDK_VERSION);

    // 业务错误 → isError + 分类文本；_meta.xyz.error 带 kind。
    let msgs = mcp_stdio(
        &[],
        &[
            init(1, "2025-11-25"),
            initialized(),
            tool_call(2, "user.rm", json!({"name": "bob"})),
        ],
    );
    let r = &by_id(&msgs, 2)["result"];
    assert_eq!(r["isError"], true, "{r}");
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("not found")
    );
    assert_eq!(r["_meta"]["xyz"]["error"]["kind"], "not_found");
}

#[test]
fn mcp_stdio_versions_blocks_and_unions() {
    // 版本门控：钉 2025-03-26 → 握手回显该版本且不带 outputSchema。
    let msgs = mcp_stdio(
        &["--versions", "2025-03-26"],
        &[
            init(1, "2025-11-25"),
            initialized(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        ],
    );
    assert_eq!(by_id(&msgs, 1)["result"]["protocolVersion"], "2025-03-26");
    let tools = by_id(&msgs, 2)["result"]["tools"].as_array().unwrap();
    assert!(
        tools.iter().all(|t| t.get("outputSchema").is_none()),
        "revision gate"
    );
    // structuredContent 同样不发射。
    let msgs = mcp_stdio(
        &["--versions", "2025-03-26"],
        &[
            init(1, "2025-03-26"),
            initialized(),
            tool_call(2, "math.sum", json!({"a": 1, "b": 2})),
        ],
    );
    let r = &by_id(&msgs, 2)["result"];
    assert!(r.get("structuredContent").is_none(), "{r}");
    assert!(r["content"][0]["text"].as_str().is_some());

    // 内容块：Content 保真 + 信封进 structuredContent。
    let msgs = mcp_stdio(
        &[],
        &[
            init(1, "2025-11-25"),
            initialized(),
            tool_call(2, "blocks.show", json!({})),
        ],
    );
    let r = &by_id(&msgs, 2)["result"];
    let content = r["content"].as_array().expect("content array");
    assert_eq!(content.len(), 2, "{r}");
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    assert_eq!(r["structuredContent"]["content"][1]["type"], "image");

    // 联合：合法分支走通，坏 tag → 分类错误。
    let msgs = mcp_stdio(
        &[],
        &[
            init(1, "2025-11-25"),
            initialized(),
            tool_call(
                2,
                "pick.by",
                json!({"sel": {"kind": "ByName", "name": "zed"}}),
            ),
            tool_call(3, "pick.by", json!({"sel": {"kind": "Nope"}})),
        ],
    );
    let ok = &by_id(&msgs, 2)["result"];
    assert!(
        ok.get("structuredContent").is_none(),
        "string result: no structuredContent: {ok}"
    );
    assert_eq!(ok["content"][0]["text"], "name=zed");
    let bad = &by_id(&msgs, 3)["result"];
    assert_eq!(bad["isError"], true, "{bad}");
}

// ================================================================ MCP http

const MCP_HEADERS: &[(&str, &str)] = &[
    ("Content-Type", "application/json"),
    ("Accept", "application/json, text/event-stream"),
];

#[test]
fn mcp_http_legacy_session_and_stateless() {
    // legacy 会话：initialize → 会话 id → tools/list。
    let srv = start("mcp", &["http"]);
    let r = http(
        srv.port,
        "POST",
        "/mcp",
        MCP_HEADERS,
        &init(1, "2025-11-25").to_string(),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let hello = &r.sse_payloads()[0]["result"];
    assert!(hello["capabilities"]["tools"].is_object(), "{hello}");
    let sid = r.header("mcp-session-id").expect("session id").to_string();

    let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
    let r = http(
        srv.port,
        "POST",
        "/mcp",
        &[
            MCP_HEADERS[0],
            MCP_HEADERS[1],
            ("mcp-session-id", &sid),
            ("MCP-Protocol-Version", "2025-11-25"),
        ],
        &list.to_string(),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let payloads = r.sse_payloads();
    let tools = payloads[0]["result"]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 14);

    // 会话 DELETE（rmcp 现状：两次都 202，已记录为上游限制）。
    let del1 = http(srv.port, "DELETE", "/mcp", &[("mcp-session-id", &sid)], "");
    let del2 = http(srv.port, "DELETE", "/mcp", &[("mcp-session-id", &sid)], "");
    assert_eq!((del1.status, del2.status), (202, 202));

    // stateless + 2026-07-28：discover 需 MCP-Protocol-Version/Mcp-Method 头与 _meta。
    let st = start("mcp", &["http", "--stateless"]);
    let discover = json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{}}}});
    let r = http(
        st.port,
        "POST",
        "/mcp",
        &[
            MCP_HEADERS[0],
            MCP_HEADERS[1],
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "server/discover"),
        ],
        &discover.to_string(),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let d = &r.sse_payloads()[0]["result"];
    assert_eq!(
        d["supportedVersions"].as_array().map(|v| v.len()),
        Some(5),
        "{d}"
    );
    assert!(d["capabilities"]["tools"].is_object(), "{d}");

    // --json-response 只在 stateless 下生效。
    let jr = start("mcp", &["http", "--stateless", "--json-response"]);
    let r = http(
        jr.port,
        "POST",
        "/mcp",
        &[
            MCP_HEADERS[0],
            MCP_HEADERS[1],
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "server/discover"),
        ],
        &discover.to_string(),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert!(
        r.json()["result"]["supportedVersions"].is_array(),
        "{}",
        r.body
    );
}

#[test]
fn mcp_http_json_response_without_stateless_warns() {
    let port = free_port();
    let mut child = Command::new(bin())
        .args([
            "mcp",
            "http",
            "--addr",
            &format!("127.0.0.1:{port}"),
            "--json-response",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp http");
    // 等端口就绪后杀掉，读回 stderr 的告警。
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(200));
    let _ = child.kill();
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let _ = child.wait();
    assert!(err.contains("--json-response"), "stderr: {err}");
    assert!(err.contains("--stateless"), "stderr: {err}");
}
