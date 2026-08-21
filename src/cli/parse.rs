// flag 定义与解析：长短名、= 形式、bool 无值形式、-- 之后全部转
// 位置参数。未知 flag 报错（用法错误 → 退出码 2）。

use crate::errors;
use crate::spec::field::FieldMeta;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    Str,
    Bool,
    Slice,
}

/// 描述一个长/短 flag 的定义与取值方式。
#[derive(Debug, Clone)]
pub struct FlagDef {
    pub long: String,
    pub short: Option<char>,
    pub kind: FlagKind,
    pub field: FieldMeta,
}

/// 一次解析中某个 flag 的取值状态。
#[derive(Debug, Clone, Default)]
pub struct FlagVal {
    pub seen: bool,
    pub str: String,
    pub list: Vec<String>,
    pub boolean: bool,
}

/// 解析 args 中的 flag，返回每个 flag 的取值与剩余位置参数。
pub fn parse_flags(
    defs: &[FlagDef],
    args: &[String],
) -> errors::Result<(Vec<FlagVal>, Vec<String>)> {
    let mut long_idx: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut short_idx: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    for (i, d) in defs.iter().enumerate() {
        long_idx.insert(d.long.as_str(), i);
        if let Some(sh) = d.short {
            short_idx.insert(sh, i);
        }
    }
    let mut fvals: Vec<FlagVal> = defs.iter().map(|_| FlagVal::default()).collect();
    let mut pos: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            pos.extend_from_slice(&args[i + 1..]);
            break;
        }
        if let Some(rest) = a.strip_prefix("--") {
            let (name, has_val, val) = match rest.split_once('=') {
                Some((n, v)) => (n, true, v.to_string()),
                None => (rest, false, String::new()),
            };
            let Some(&di) = long_idx.get(name) else {
                return Err(errors::Error::new(
                    errors::Kind::InvalidInput,
                    format!("unknown flag: --{name}"),
                ));
            };
            let fv = &mut fvals[di];
            if defs[di].kind == FlagKind::Bool {
                let mut b = true;
                if has_val {
                    b = crate::spec::scalar::parse_bool(&val).map_err(|_| {
                        errors::Error::new(
                            errors::Kind::InvalidInput,
                            format!("invalid boolean value {val:?} for --{name}"),
                        )
                    })?;
                }
                fv.seen = true;
                fv.boolean = b;
                i += 1;
                continue;
            }
            let val = if !has_val {
                if i + 1 >= args.len() {
                    return Err(errors::Error::new(
                        errors::Kind::InvalidInput,
                        format!("flag needs an argument: --{name}"),
                    ));
                }
                i += 1;
                args[i].clone()
            } else {
                val
            };
            fv.seen = true;
            if defs[di].kind == FlagKind::Slice {
                fv.list.push(val);
            } else {
                fv.str = val;
            }
            i += 1;
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            let chars: Vec<char> = a.chars().collect();
            let sh = chars[1];
            let Some(&di) = short_idx.get(&sh) else {
                return Err(errors::Error::new(
                    errors::Kind::InvalidInput,
                    format!("unknown shorthand flag: -{sh}"),
                ));
            };
            let fv = &mut fvals[di];
            let mut rest: String = chars.iter().skip(2).collect();
            if let Some(stripped) = rest.strip_prefix('=') {
                rest = stripped.to_string();
            }
            if defs[di].kind == FlagKind::Bool {
                if rest.is_empty() {
                    fv.seen = true;
                    fv.boolean = true;
                    i += 1;
                    continue;
                }
                let b = crate::spec::scalar::parse_bool(&rest).map_err(|_| {
                    errors::Error::new(
                        errors::Kind::InvalidInput,
                        format!("invalid boolean value {rest:?} for -{sh}"),
                    )
                })?;
                fv.seen = true;
                fv.boolean = b;
                i += 1;
                continue;
            }
            let rest = if rest.is_empty() {
                if i + 1 >= args.len() {
                    return Err(errors::Error::new(
                        errors::Kind::InvalidInput,
                        format!("flag needs an argument: -{sh}"),
                    ));
                }
                i += 1;
                args[i].clone()
            } else {
                rest
            };
            fv.seen = true;
            if defs[di].kind == FlagKind::Slice {
                fv.list.push(rest);
            } else {
                fv.str = rest;
            }
            i += 1;
            continue;
        }
        pos.push(a.clone());
        i += 1;
    }
    Ok((fvals, pos))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::field::{FieldKind, FieldMeta};

    fn str_def(name: &str, short: Option<char>) -> FlagDef {
        FlagDef {
            long: name.to_string(),
            short,
            kind: FlagKind::Str,
            field: fm(name),
        }
    }
    fn bool_def(name: &str, short: Option<char>) -> FlagDef {
        FlagDef {
            long: name.to_string(),
            short,
            kind: FlagKind::Bool,
            field: fm(name),
        }
    }
    fn slice_def(name: &str, short: Option<char>) -> FlagDef {
        FlagDef {
            long: name.to_string(),
            short,
            kind: FlagKind::Slice,
            field: fm(name),
        }
    }
    fn fm(name: &str) -> FieldMeta {
        FieldMeta {
            name: name.to_string(),
            json_name: name.to_string(),
            kind: FieldKind::String,
            ..Default::default()
        }
    }
    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn long_short_and_eq_forms() {
        let defs = vec![str_def("query", Some('q')), bool_def("verbose", Some('v'))];
        let (vals, pos) =
            parse_flags(&defs, &args(&["--query", "x", "-v", "--query=y", "p"])).unwrap();
        assert!(vals[0].seen);
        assert_eq!(vals[0].str, "y"); // 后值覆盖
        assert!(vals[1].boolean);
        assert_eq!(pos, vec!["p".to_string()]);
    }

    #[test]
    fn slice_accumulates() {
        let defs = vec![slice_def("tags", Some('t'))];
        let (vals, _) = parse_flags(&defs, &args(&["--tags", "a", "-t", "b"])).unwrap();
        assert_eq!(vals[0].list, vec!["a", "b"]);
    }

    #[test]
    fn bool_with_value() {
        let defs = vec![bool_def("v", None)];
        let (vals, _) = parse_flags(&defs, &args(&["--v=false"])).unwrap();
        assert!(!vals[0].boolean);
        let (vals, _) = parse_flags(&defs, &args(&["--v"])).unwrap();
        assert!(vals[0].boolean);
    }

    #[test]
    fn double_dash_stops_parsing() {
        let defs = vec![str_def("a", None)];
        let (vals, pos) = parse_flags(&defs, &args(&["--a", "1", "--", "--a", "2"])).unwrap();
        assert_eq!(vals[0].str, "1");
        assert_eq!(pos, vec!["--a".to_string(), "2".to_string()]);
    }

    #[test]
    fn unknown_flags_error() {
        let defs = vec![str_def("a", Some('a'))];
        let err = parse_flags(&defs, &args(&["--nope"])).unwrap_err();
        assert!(err.to_string().contains("unknown flag: --nope"), "{err}");
        let err = parse_flags(&defs, &args(&["-z"])).unwrap_err();
        assert!(
            err.to_string().contains("unknown shorthand flag: -z"),
            "{err}"
        );
        let err = parse_flags(&defs, &args(&["--a"])).unwrap_err();
        assert!(
            err.to_string().contains("flag needs an argument: --a"),
            "{err}"
        );
    }

    #[test]
    fn shorthand_with_attached_value() {
        let defs = vec![str_def("name", Some('n'))];
        let (vals, _) = parse_flags(&defs, &args(&["-nalice"])).unwrap();
        assert_eq!(vals[0].str, "alice");
        let (vals, _) = parse_flags(&defs, &args(&["-n=alice"])).unwrap();
        assert_eq!(vals[0].str, "alice");
    }
}
