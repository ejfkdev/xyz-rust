// mcp 模式的参数解析（Go args.go 对应物）：<transport> + flags。
// 与 Go 版差异：sse 传输词仍被接受解析，但派发时以清晰错误拒绝
// （官方 Rust SDK 已随 2026-07-28 修订移除 HTTP+SSE 传输）。

use std::time::Duration;

use crate::errors;
use crate::mcp::Options;

pub fn parse_args(args: &[String]) -> errors::Result<(String, Options)> {
    let mut opts = Options::default();
    let mut transport = String::new();
    let mut positional = 0usize;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--json-response" => opts.json_response = true,
            "--stateless" => opts.stateless = true,
            _ if a.starts_with("--addr=") => {
                opts.addr = a.trim_start_matches("--addr=").to_string()
            }
            "--addr" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                opts.addr = v.clone();
            }
            _ if a.starts_with("--versions=") => {
                opts.versions =
                    crate::builtins::split_versions(a.trim_start_matches("--versions="));
            }
            "--versions" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                opts.versions = crate::builtins::split_versions(v);
            }
            "--bearer" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                opts.bearer_tokens = crate::builtins::merge_tokens(opts.bearer_tokens.clone(), v);
            }
            _ if a.starts_with("--bearer=") => {
                opts.bearer_tokens = crate::builtins::merge_tokens(
                    opts.bearer_tokens.clone(),
                    a.trim_start_matches("--bearer="),
                );
            }
            "--session-timeout" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                opts.session_timeout = crate::spec::scalar::parse_duration(v).map_err(|e| {
                    errors::Error::new(
                        errors::Kind::Internal,
                        format!("invalid --session-timeout {v:?}: {e}"),
                    )
                })?;
            }
            _ if a.starts_with("--session-timeout=") => {
                let v = a.trim_start_matches("--session-timeout=");
                opts.session_timeout = crate::spec::scalar::parse_duration(v).map_err(|e| {
                    errors::Error::new(
                        errors::Kind::Internal,
                        format!("invalid --session-timeout: {e}"),
                    )
                })?;
            }
            "--cors" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                opts.cors_origins = crate::builtins::merge_tokens(opts.cors_origins.clone(), v);
            }
            _ if a.starts_with("--cors=") => {
                opts.cors_origins = crate::builtins::merge_tokens(
                    opts.cors_origins.clone(),
                    a.trim_start_matches("--cors="),
                );
            }
            _ if a.starts_with("--name=") => {
                opts.name = a.trim_start_matches("--name=").to_string()
            }
            "--name" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                opts.name = v.clone();
            }
            _ if a.starts_with("--server-version=") => {
                opts.version = a.trim_start_matches("--server-version=").to_string()
            }
            "--server-version" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                opts.version = v.clone();
            }
            _ if a.starts_with('-') => {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!("unknown flag {a:?}"),
                ));
            }
            _ => {
                if positional == 0 {
                    transport = a.to_string();
                }
                positional += 1;
            }
        }
        i += 1;
    }
    if transport.is_empty() {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            "missing transport".to_string(),
        ));
    }
    if positional > 1 {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!(
                "unexpected argument {:?}",
                args.last().map(String::as_str).unwrap_or("")
            ),
        ));
    }
    let _ = Duration::ZERO;
    Ok((transport, opts))
}

impl Options {
    /// 把预设选项（如根派发器注入的 --xyz.bearer/--xyz.addr）作为默认值：
    /// 命令行 flag 已设置的字段优先，布尔项取或（未提供关闭旗标）。
    pub(crate) fn merge_defaults(&mut self, base: &Options) {
        if self.addr.is_empty() {
            self.addr = base.addr.clone();
        }
        if self.name.is_empty() {
            self.name = base.name.clone();
        }
        if self.version.is_empty() {
            self.version = base.version.clone();
        }
        if self.versions.is_empty() {
            self.versions = base.versions.clone();
        }
        if self.instructions.is_empty() {
            self.instructions = base.instructions.clone();
        }
        if self.bearer_tokens.is_empty() {
            self.bearer_tokens = base.bearer_tokens.clone();
        }
        if self.session_timeout.is_zero() {
            self.session_timeout = base.session_timeout;
        }
        if self.cors_origins.is_empty() {
            self.cors_origins = base.cors_origins.clone();
        }
        self.json_response = self.json_response || base.json_response;
        self.stateless = self.stateless || base.stateless;
    }
}
