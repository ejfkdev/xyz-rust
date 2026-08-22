// 内置参数解析：全局 --xyz.*（任意位置）与 serve 模式的裸名 flag（模式词
// 即命名空间），统一折叠进 Config。优先级：局部 flag > 全局/代码 Config >
// 默认。

use crate::config::Config;
use crate::errors;

/// 提取 --xyz.* 内置参数，把它们从参数列表中移除并写回 cfg；命令行值
/// 覆盖代码里的 Config。剩余参数原样返回。
pub fn strip_xyz_flags(args: Vec<String>, cfg: &mut Config) -> errors::Result<Vec<String>> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if a == "--" {
            // 终止符之后全是位置参数：不再剥内置参数，原样保留。
            out.extend(args[i..].iter().cloned());
            break;
        }
        let matched = match a.as_str() {
            "--xyz.addr" => {
                cfg.addr = take_value(&args, &mut i, &a)?;
                true
            }
            "--xyz.bearer" => {
                let v = take_value(&args, &mut i, &a)?;
                cfg.bearer_tokens = merge_tokens(cfg.bearer_tokens.clone(), &v);
                true
            }
            "--xyz.log-level" => {
                let v = take_value(&args, &mut i, &a)?;
                cfg.log_level = crate::logx::parse_level(&v)?;
                true
            }
            "--xyz.timeout" => {
                let v = take_value(&args, &mut i, &a)?;
                cfg.timeout = crate::spec::scalar::parse_duration(&v).map_err(|e| {
                    errors::Error::new(
                        errors::Kind::Internal,
                        format!("invalid --xyz.timeout {v:?}: {e}"),
                    )
                })?;
                true
            }
            "--xyz.tls-cert" => {
                cfg.cert_file = take_value(&args, &mut i, &a)?;
                true
            }
            "--xyz.tls-key" => {
                cfg.key_file = take_value(&args, &mut i, &a)?;
                true
            }
            "--xyz.cors" => {
                let v = take_value(&args, &mut i, &a)?;
                cfg.cors_origins = merge_tokens(cfg.cors_origins.clone(), &v);
                true
            }
            "--xyz.lang" => {
                let v = take_value(&args, &mut i, &a)?;
                if crate::lang::XyzLang::parse(&v).is_none() {
                    return Err(errors::Error::new(
                        errors::Kind::Internal,
                        format!("invalid --xyz.lang {v:?} (want en|zh-CN)"),
                    ));
                }
                cfg.lang = v;
                true
            }
            _ => {
                // = 形式
                if let Some(v) = a.strip_prefix("--xyz.addr=") {
                    cfg.addr = v.to_string();
                    true
                } else if let Some(v) = a.strip_prefix("--xyz.bearer=") {
                    cfg.bearer_tokens = merge_tokens(cfg.bearer_tokens.clone(), v);
                    true
                } else if let Some(v) = a.strip_prefix("--xyz.log-level=") {
                    cfg.log_level = crate::logx::parse_level(v)?;
                    true
                } else if let Some(v) = a.strip_prefix("--xyz.timeout=") {
                    cfg.timeout = crate::spec::scalar::parse_duration(v).map_err(|e| {
                        errors::Error::new(
                            errors::Kind::Internal,
                            format!("invalid --xyz.timeout: {e}"),
                        )
                    })?;
                    true
                } else if let Some(v) = a.strip_prefix("--xyz.tls-cert=") {
                    cfg.cert_file = v.to_string();
                    true
                } else if let Some(v) = a.strip_prefix("--xyz.tls-key=") {
                    cfg.key_file = v.to_string();
                    true
                } else if let Some(v) = a.strip_prefix("--xyz.cors=") {
                    cfg.cors_origins = merge_tokens(cfg.cors_origins.clone(), v);
                    true
                } else if let Some(v) = a.strip_prefix("--xyz.lang=") {
                    if crate::lang::XyzLang::parse(v).is_none() {
                        return Err(errors::Error::new(
                            errors::Kind::Internal,
                            format!("invalid --xyz.lang {v:?} (want en|zh-CN)"),
                        ));
                    }
                    cfg.lang = v.to_string();
                    true
                } else {
                    false
                }
            }
        };
        if !matched {
            out.push(a);
        }
        i += 1;
    }
    Ok(out)
}

fn take_value(args: &[String], i: &mut usize, flag: &str) -> errors::Result<String> {
    if *i + 1 >= args.len() {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!("flag {flag} needs an argument"),
        ));
    }
    *i += 1;
    Ok(args[*i].clone())
}

/// serve 模式的裸名 flag 解析（--addr/--bearer/--timeout/--tls-*/--cors）；
/// 全局 --xyz.* 与代码 Config 已由根派发器折叠进 cfg。
pub fn parse_serve_args(args: &[String], mut cfg: Config) -> Config {
    if cfg.addr.is_empty() {
        cfg.addr = ":8080".to_string();
    }
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--addr" | "--bearer" | "--timeout" | "--tls-cert" | "--tls-key" | "--cors" => {
                let Some(v) = args.get(i + 1) else { break };
                i += 1;
                match a {
                    "--addr" => cfg.addr = v.clone(),
                    "--bearer" => cfg.bearer_tokens = merge_tokens(cfg.bearer_tokens.clone(), v),
                    "--timeout" => {
                        if let Ok(d) = crate::spec::scalar::parse_duration(v) {
                            cfg.timeout = d;
                        }
                    }
                    "--tls-cert" => cfg.cert_file = v.clone(),
                    "--tls-key" => cfg.key_file = v.clone(),
                    _ => cfg.cors_origins = merge_tokens(cfg.cors_origins.clone(), v),
                }
            }
            other => {
                if let Some(v) = other.strip_prefix("--addr=") {
                    cfg.addr = v.to_string();
                } else if let Some(v) = other.strip_prefix("--bearer=") {
                    cfg.bearer_tokens = merge_tokens(cfg.bearer_tokens.clone(), v);
                } else if let Some(v) = other.strip_prefix("--timeout=") {
                    if let Ok(d) = crate::spec::scalar::parse_duration(v) {
                        cfg.timeout = d;
                    }
                } else if let Some(v) = other.strip_prefix("--tls-cert=") {
                    cfg.cert_file = v.to_string();
                } else if let Some(v) = other.strip_prefix("--tls-key=") {
                    cfg.key_file = v.to_string();
                } else if let Some(v) = other.strip_prefix("--cors=") {
                    cfg.cors_origins = merge_tokens(cfg.cors_origins.clone(), v);
                }
            }
        }
        i += 1;
    }
    cfg
}

/// 逗号分隔列表的解析（--versions 等）。
pub fn split_versions(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// 合并逗号分隔的 token 列表并去重（代码预置在前，命令行追加在后）。
pub fn merge_tokens(existing: Vec<String>, flag: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for t in existing {
        if !seen.contains(&t) {
            seen.push(t.clone());
            out.push(t);
        }
    }
    for t in flag.split(',').map(str::trim) {
        if !t.is_empty() && !seen.iter().any(|s| s == t) {
            seen.push(t.to_string());
            out.push(t.to_string());
        }
    }
    out
}
