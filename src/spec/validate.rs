// 库内 validator：go-playground/validator 语法的兼容子集，零第三方依赖。
// 不支持的规则在注册期（xyz_meta 解析）报错——与「注册期即报错」原则
// 一致，而不是运行时悄悄忽略。

use crate::errors;
use crate::spec::XyzField;
use crate::spec::field::FieldMeta;

/// 一条已解析的校验规则；num 是数值参数（-1 表示非数值规则）。
#[derive(Debug, Clone, PartialEq)]
pub struct VRule {
    pub key: String,
    pub args: Vec<String>,
    pub num: f64,
}

/// 带一个数值参数的规则集合。
pub const NUMERIC_RULE_KEYS: &[&str] = &["min", "max", "len", "gt", "gte", "lt", "lte"];

/// 本实现支持的规则全集。
pub const SUPPORTED_RULE_KEYS: &[&str] = &[
    "required",
    "omitempty",
    "min",
    "max",
    "len",
    "gt",
    "gte",
    "lt",
    "lte",
    "oneof",
    "email",
];

/// 解析 validate tag。规则逗号分隔，参数用 = 或空格给出
/// （与 go-playground/validator 一致："required,min=2,oneof=fast slow"）。
pub fn parse_validate_tag(v: &str) -> errors::Result<Vec<VRule>> {
    if v.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for part in v.split(',').map(str::trim) {
        if part.is_empty() {
            continue;
        }
        let (key, rest) = match part.split_once('=') {
            Some((k, r)) => (k.trim(), r),
            None => (part, ""),
        };
        if !SUPPORTED_RULE_KEYS.contains(&key) {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!(
                    "unsupported validate rule {key:?} (supported: required, omitempty, min, max, len, gt, gte, lt, lte, oneof, email)"
                ),
            ));
        }
        let mut rule = VRule {
            key: key.to_string(),
            args: rest.split_whitespace().map(str::to_string).collect(),
            num: -1.0,
        };
        if NUMERIC_RULE_KEYS.contains(&key) {
            if rule.args.len() != 1 {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!(
                        "validate rule {key:?} needs exactly one numeric argument, got {rest:?}"
                    ),
                ));
            }
            let n = rule.args[0].parse::<f64>().map_err(|_| {
                errors::Error::new(
                    errors::Kind::Internal,
                    format!("validate rule {key:?}: {:?} is not a number", rule.args[0]),
                )
            })?;
            rule.num = n;
            rule.args.clear(); // 数值参数不进 args（oneof 专用）
        }
        out.push(rule);
    }
    Ok(out)
}

/// 构造字段规则错误（Go fieldRuleError 的对应物）。
pub fn field_rule_error(f: &FieldMeta, rule: &str) -> errors::Error {
    let name = if f.json_name.is_empty() {
        f.name.as_str()
    } else {
        f.json_name.as_str()
    };
    errors::Error::new(
        errors::Kind::InvalidInput,
        format!("invalid value for field {name:?}: {rule}"),
    )
}

/// 对单值字段跑规则集（omitempty 空的短路、required 的零值拒绝）。
/// 宏生成的 xyz_validate 对每个叶子字段调用；嵌套与容器由宏生成代码
/// 自行递归。
pub fn check_rules<T: XyzField>(f: &FieldMeta, v: &T) -> errors::Result<()> {
    if f.rules.is_empty() {
        return Ok(());
    }
    let zero = v.xyz_is_zero();
    for r in &f.rules {
        match r.key.as_str() {
            "omitempty" => {
                if zero {
                    break; // 空值：跳过本字段全部校验
                }
                continue;
            }
            "required" => {
                if zero {
                    return Err(field_rule_error(f, "required"));
                }
                continue;
            }
            _ => {
                if !v.xyz_rule_ok(r) {
                    return Err(field_rule_error(f, &r.key));
                }
            }
        }
    }
    Ok(())
}

/// Option<T> 字段的规则检查（Go 指针解引用语义：nil 对 min/max/len
/// 判假、required 判零）。
pub fn check_opt_rules<T: XyzField>(f: &FieldMeta, v: Option<&T>) -> errors::Result<()> {
    if f.rules.is_empty() {
        return Ok(());
    }
    let zero = v.is_none();
    for r in &f.rules {
        match r.key.as_str() {
            "omitempty" => {
                if zero {
                    break;
                }
                continue;
            }
            "required" => {
                if zero {
                    return Err(field_rule_error(f, "required"));
                }
                continue;
            }
            _ => match v {
                Some(inner) => {
                    if !inner.xyz_rule_ok(r) {
                        return Err(field_rule_error(f, &r.key));
                    }
                }
                None => {
                    return Err(field_rule_error(f, &r.key));
                }
            },
        }
    }
    Ok(())
}

/// Vec<T> 字段的规则检查（min/max/len 按长度；oneof 按元素 %v 形态）。
pub fn check_vec_rules<T: XyzField>(f: &FieldMeta, v: &[T]) -> errors::Result<()> {
    if f.rules.is_empty() {
        return Ok(());
    }
    let zero = v.is_empty();
    let fmt = format!(
        "[{}]",
        v.iter().map(|x| x.xyz_fmt()).collect::<Vec<_>>().join(" ")
    );
    for r in &f.rules {
        match r.key.as_str() {
            "omitempty" => {
                if zero {
                    break;
                }
                continue;
            }
            "required" => {
                if zero {
                    return Err(field_rule_error(f, "required"));
                }
                continue;
            }
            "min" | "max" | "len" => {
                let n = v.len() as f64;
                let ok = match r.key.as_str() {
                    "min" => n >= r.num,
                    "max" => n <= r.num,
                    _ => n == r.num,
                };
                if !ok {
                    return Err(field_rule_error(f, &r.key));
                }
            }
            "oneof" => {
                if !r.args.iter().any(|a| a == &fmt) {
                    return Err(field_rule_error(f, &r.key));
                }
            }
            _ => {
                return Err(field_rule_error(f, &r.key));
            }
        }
    }
    Ok(())
}
/// 容器的元素元数据路径（Vec<T>/Option<T> 嵌套校验用）；无元素时 None。
pub fn elem_ref(f: &FieldMeta) -> Option<&FieldMeta> {
    f.elem.as_deref()
}
