// 标量转换机制：Go 侧 scalarValue/rawInt64/rawUint64/rawFloat64/
// time.ParseDuration/strconv.ParseBool 与 numericOf/isZero 的 Rust 对应物，
// 全部零第三方依赖手写。数值转换带无损检查（"3.7" 永不静默变成整数）。

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Number, Value};

use crate::errors;
use crate::spec::field::synthetic;
use crate::spec::schema::Schema;
use crate::spec::validate::VRule;
use crate::spec::{FieldKind, FieldSpec, XyzField, XyzSchema};

pub(crate) fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Go strconv.ParseBool 的接受集。
pub fn parse_bool(s: &str) -> Result<bool, String> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("expect boolean, got {s:?}")),
    }
}

/// Go time.ParseDuration 的语法：复合单位、小数、负号，支持 us/µs/μs/ns/
/// ms/s/m/h。经 f64 累加（Go 的小数路径同语义）。
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let src = s.trim();
    if src.is_empty() {
        return Err(format!("invalid duration {s:?}"));
    }
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if bytes[0] == b'-' {
        neg = true;
        i += 1;
    } else if bytes[0] == b'+' {
        i += 1;
    }
    if i == bytes.len() {
        return Err(format!("invalid duration {s:?}"));
    }
    let mut total: f64 = 0.0;
    let mut any = false;
    while i < bytes.len() {
        // 数值部分：整数 + 可选小数
        let num_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        if num_start == i {
            return Err(format!(
                "invalid duration {s:?}: expected number at position {i}"
            ));
        }
        let numstr = &src[num_start..i];
        let num: f64 = numstr
            .parse()
            .map_err(|_| format!("invalid duration {s:?}"))?;
        // 单位部分：双字符单位（ns/us/ms 与 Unicode µs/μs）优先，其次
        // 单字符（s/m/h）。多字符单位必须整体吃掉（"100ms" 不能剩 's'）。
        let unit_start = i;
        let rest = &src[unit_start..];
        let (unit, mult): (&str, f64) = if rest.starts_with("ns") {
            ("ns", 1.0)
        } else if rest.starts_with("us") {
            ("us", 1e3)
        } else if rest.starts_with("\u{00B5}s") {
            ("\u{00B5}s", 1e3)
        } else if rest.starts_with("\u{03BC}s") {
            ("\u{03BC}s", 1e3)
        } else if rest.starts_with("ms") {
            ("ms", 1e6)
        } else if rest.starts_with('s') {
            ("s", 1e9)
        } else if rest.starts_with('m') {
            ("m", 60e9)
        } else if rest.starts_with('h') {
            ("h", 3600e9)
        } else {
            return Err(format!(
                "invalid duration {s:?}: unknown unit at position {i}"
            ));
        };
        i += unit.len();
        total += num * mult;
        any = true;
    }
    if !any {
        return Err(format!("invalid duration {s:?}"));
    }
    let nanos = if neg { -total.round() } else { total.round() };
    if !nanos.is_finite() || nanos < 0.0 || nanos > u64::MAX as f64 {
        return Err(format!(
            "invalid duration {s:?}: out of range (Rust 侧负数 Duration 不支持)"
        ));
    }
    Ok(Duration::from_nanos(nanos as u64))
}

/// Go time.Duration.String 的输出形态（"300ms"、"1.5h"、"1h2m3.5s"）。
pub fn format_duration(d: Duration) -> String {
    let nanos = d.as_nanos() as f64;
    if nanos == 0.0 {
        return "0s".to_string();
    }
    let mut out = String::new();
    let h = (nanos / 3600e9).floor();
    let m = ((nanos - h * 3600e9) / 60e9).floor();
    let s = (nanos - h * 3600e9 - m * 60e9) / 1e9;
    if h == 0.0 && m == 0.0 && nanos < 1e9 {
        // Go：不足 1s 用毫秒表达（"300ms" 而非 "0.3s"）。
        let ms = nanos / 1e6;
        out.push_str(&trim_frac(&format!("{:.3}", ms)));
        out.push_str("ms");
    } else if h > 0.0 || m > 0.0 {
        out.push_str(&format!("{}h{}m", h, m));
        push_frac_secs(&mut out, s);
    } else if s != 0.0 {
        push_frac_secs(&mut out, s);
    } else {
        out.push_str("0s");
    }
    out
}

fn trim_frac(s: &str) -> String {
    let s = s.trim_end_matches('0').to_string();
    s.trim_end_matches('.').to_string()
}

fn push_frac_secs(out: &mut String, s: f64) {
    let whole = s.floor();
    let frac = s - whole;
    if frac == 0.0 {
        out.push_str(&format!("{}s", whole));
    } else {
        let mut fs = format!("{:.9}", s);
        while fs.ends_with('0') {
            fs.pop();
        }
        if fs.ends_with('.') {
            fs.pop();
        }
        out.push_str(&fs);
        out.push('s');
    }
}

/// RFC3339 解析（chrono 承担格式细节；统一归一为 Utc）。
pub fn parse_datetime(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| format!("expect RFC3339 string, got {s:?}: {e}"))
}

/// Go emailRe `^[^@\s]+@[^@\s]+\.[^@\s]+$` 的手写等价。
pub fn email_ok(s: &str) -> bool {
    if s.is_empty() || s.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let mut parts = s.split('@');
    let local = parts.next().unwrap_or("");
    let rest = match parts.next() {
        Some(r) if parts.next().is_none() => r,
        _ => return false,
    };
    if local.is_empty() {
        return false;
    }
    match rest.rsplit_once('.') {
        Some((dom, tld)) => !dom.is_empty() && !tld.is_empty(),
        None => false,
    }
}

/// 按 kind 把字面量解析成 typed Value（default/enum 标签与 CLI 字符串
/// 进 decode 前共享的路径）。
pub fn parse_scalar_literal(kind: FieldKind, s: &str) -> errors::Result<Value> {
    match kind {
        FieldKind::String => Ok(Value::String(s.to_string())),
        FieldKind::Bool => parse_bool(s)
            .map(Value::Bool)
            .map_err(|e| errors::Error::new(errors::Kind::Internal, e)),
        FieldKind::I8
        | FieldKind::I16
        | FieldKind::I32
        | FieldKind::I64
        | FieldKind::U8
        | FieldKind::U16
        | FieldKind::U32
        | FieldKind::U64 => {
            let (i, u): (Option<i64>, Option<u64>) = match kind.is_signed_width() {
                Some(_) => {
                    let i = raw_i64(&Value::String(s.to_string()))
                        .map_err(|e| errors::Error::new(errors::Kind::Internal, e))?;
                    check_int_width(kind, i)?;
                    (Some(i), None)
                }
                None => {
                    let u = raw_u64(&Value::String(s.to_string()))
                        .map_err(|e| errors::Error::new(errors::Kind::Internal, e))?;
                    check_uint_width(kind, u)?;
                    (None, Some(u))
                }
            };
            Ok(match u {
                Some(u) => Value::Number(Number::from(u)),
                None => Value::Number(Number::from(i.unwrap())),
            })
        }
        FieldKind::F32 | FieldKind::F64 => {
            let f = s.parse::<f64>().map_err(|_| {
                errors::Error::new(errors::Kind::Internal, format!("expect number, got {s:?}"))
            })?;
            let f = match kind {
                FieldKind::F32 => f as f32 as f64, // f32 按宽度截断（溢出检查略，NaN 同 Go 行为）
                _ => f,
            };
            Number::from_f64(f).map(Value::Number).ok_or_else(|| {
                errors::Error::new(errors::Kind::Internal, format!("expect number, got {s:?}"))
            })
        }
        _ => Err(errors::Error::new(
            errors::Kind::Internal,
            format!("unsupported scalar kind {kind:?} for literal {s:?}"),
        )),
    }
}

impl FieldKind {
    pub(crate) fn is_signed_width(&self) -> Option<u32> {
        match self {
            FieldKind::I8 => Some(8),
            FieldKind::I16 => Some(16),
            FieldKind::I32 => Some(32),
            FieldKind::I64 => Some(64),
            _ => None,
        }
    }
}
pub(crate) fn check_int_width(kind: FieldKind, v: i64) -> errors::Result<()> {
    let ok = match kind.is_signed_width() {
        Some(8) => (i8::MIN as i64..=i8::MAX as i64).contains(&v),
        Some(16) => (i16::MIN as i64..=i16::MAX as i64).contains(&v),
        Some(32) => (i32::MIN as i64..=i32::MAX as i64).contains(&v),
        _ => true,
    };
    if !ok {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!("value {v} overflows {kind:?}"),
        ));
    }
    Ok(())
}

pub(crate) fn check_uint_width(kind: FieldKind, v: u64) -> errors::Result<()> {
    let ok = match kind {
        FieldKind::U8 => v <= u8::MAX as u64,
        FieldKind::U16 => v <= u16::MAX as u64,
        FieldKind::U32 => v <= u32::MAX as u64,
        _ => true,
    };
    if !ok {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!("value {v} overflows {kind:?}"),
        ));
    }
    Ok(())
}

/// Go rawInt64：接收 number（i64/u64/f64 整数）与十进制字符串，无损检查。
pub(crate) fn raw_i64(v: &Value) -> Result<i64, String> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Ok(i);
            }
            if let Some(u) = n.as_u64() {
                return i64::try_from(u).map_err(|_| format!("value {u} out of int64 range"));
            }
            let f = n
                .as_f64()
                .ok_or_else(|| format!("expect integer, got {v}"))?;
            if f != f.trunc() {
                return Err(format!("expect integer, got {v}"));
            }
            if f > i64::MAX as f64 || f < i64::MIN as f64 {
                return Err(format!("value {v} out of int64 range"));
            }
            Ok(f as i64)
        }
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("expect integer, got {s:?}")),
        _ => Err(format!("expect integer, got {}", type_name(v))),
    }
}

/// Go rawUint64。
pub(crate) fn raw_u64(v: &Value) -> Result<u64, String> {
    match v {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                return Ok(u);
            }
            if let Some(i) = n.as_i64() {
                return u64::try_from(i).map_err(|_| format!("expect unsigned integer, got {i}"));
            }
            let f = n
                .as_f64()
                .ok_or_else(|| format!("expect unsigned integer, got {v}"))?;
            if f != f.trunc() || f < 0.0 {
                return Err(format!("expect unsigned integer, got {v}"));
            }
            if f > u64::MAX as f64 {
                return Err(format!("value {v} out of u64 range"));
            }
            Ok(f as u64)
        }
        Value::String(s) => s
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("expect unsigned integer, got {s:?}")),
        _ => Err(format!("expect unsigned integer, got {}", type_name(v))),
    }
}

/// Go rawFloat64。
pub(crate) fn raw_f64(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                return Ok(f);
            }
            Err(format!("expect number, got {v}"))
        }
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("expect number, got {s:?}")),
        _ => Err(format!("expect number, got {}", type_name(v))),
    }
}

/// 单条规则的通用判定（validate.rs 的 rule_ok 入口，标量实现共用）。
pub(crate) fn rule_ok_bits(
    r: &VRule,
    len: Option<i64>,
    numeric: Option<i64>,
    s: &str,
    email: bool,
) -> bool {
    match r.key.as_str() {
        "min" | "max" | "len" => len
            .map(|n| match r.key.as_str() {
                "min" => n >= r.num as i64,
                "max" => n <= r.num as i64,
                _ => n == r.num as i64,
            })
            .unwrap_or(false),
        "gt" | "gte" | "lt" | "lte" => numeric
            .map(|n| match r.key.as_str() {
                "gt" => n > r.num as i64,
                "gte" => n >= r.num as i64,
                "lt" => n < r.num as i64,
                _ => n <= r.num as i64,
            })
            .unwrap_or(false),
        "oneof" => r.args.iter().any(|a| a == s),
        "email" => email && email_ok(s),
        _ => false,
    }
}

// ---- 标量的 XyzField / XyzSchema 实现（宏批量展开）----

macro_rules! impl_int_field {
    ($t:ty, $kind:expr, $signed:expr) => {
        impl XyzField for $t {
            fn xyz_from_value(v: &Value) -> errors::Result<Self> {
                if $signed {
                    let r = raw_i64(v)
                        .map_err(|e| errors::Error::new(errors::Kind::InvalidInput, e))?;
                    return <$t>::try_from(r).map_err(|_| {
                        errors::Error::new(
                            errors::Kind::InvalidInput,
                            format!("value {r} overflows {}", stringify!($t)),
                        )
                    });
                }
                let u =
                    raw_u64(v).map_err(|e| errors::Error::new(errors::Kind::InvalidInput, e))?;
                <$t>::try_from(u).map_err(|_| {
                    errors::Error::new(
                        errors::Kind::InvalidInput,
                        format!("value {u} overflows {}", stringify!($t)),
                    )
                })
            }
            fn xyz_zero() -> Self {
                0 as $t
            }
            fn xyz_is_zero(&self) -> bool {
                *self == 0
            }
            fn xyz_rule_ok(&self, r: &VRule) -> bool {
                let n = *self as i64;
                rule_ok_bits(r, Some(n), Some(n), &self.xyz_fmt(), false)
            }
            fn xyz_fmt(&self) -> String {
                self.to_string()
            }
            fn xyz_spec_of() -> FieldSpec {
                synthetic($kind, Vec::new(), None)
            }
        }
        impl XyzSchema for $t {
            fn xyz_schema() -> Option<Schema> {
                Some(Schema {
                    r#type: Some("integer".to_string()),
                    ..Default::default()
                })
            }
        }
    };
}

impl_int_field!(i8, FieldKind::I8, true);
impl_int_field!(i16, FieldKind::I16, true);
impl_int_field!(i32, FieldKind::I32, true);
impl_int_field!(i64, FieldKind::I64, true);
impl_int_field!(u8, FieldKind::U8, false);
impl_int_field!(u16, FieldKind::U16, false);
impl_int_field!(u32, FieldKind::U32, false);
impl_int_field!(u64, FieldKind::U64, false);

macro_rules! impl_float_field {
    ($t:ty, $kind:expr) => {
        impl XyzField for $t {
            fn xyz_from_value(v: &Value) -> errors::Result<Self> {
                let f =
                    raw_f64(v).map_err(|e| errors::Error::new(errors::Kind::InvalidInput, e))?;
                Ok(f as $t)
            }
            fn xyz_zero() -> Self {
                0.0 as $t
            }
            fn xyz_is_zero(&self) -> bool {
                *self == 0.0
            }
            fn xyz_rule_ok(&self, r: &VRule) -> bool {
                let n = *self as i64; // Go numericOf 截断语义
                rule_ok_bits(r, Some(n), Some(n), &self.xyz_fmt(), false)
            }
            fn xyz_fmt(&self) -> String {
                self.to_string()
            }
            fn xyz_spec_of() -> FieldSpec {
                synthetic($kind, Vec::new(), None)
            }
        }
        impl XyzSchema for $t {
            fn xyz_schema() -> Option<Schema> {
                Some(Schema {
                    r#type: Some("number".to_string()),
                    ..Default::default()
                })
            }
        }
    };
}

impl_float_field!(f32, FieldKind::F32);
impl_float_field!(f64, FieldKind::F64);

impl XyzField for bool {
    fn xyz_from_value(v: &Value) -> errors::Result<Self> {
        match v {
            Value::Bool(b) => Ok(*b),
            Value::String(s) => {
                parse_bool(s).map_err(|e| errors::Error::new(errors::Kind::InvalidInput, e))
            }
            _ => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("expect boolean, got {}", type_name(v)),
            )),
        }
    }
    fn xyz_zero() -> Self {
        false
    }
    fn xyz_is_zero(&self) -> bool {
        !*self
    }
    fn xyz_rule_ok(&self, r: &VRule) -> bool {
        rule_ok_bits(r, None, None, &self.xyz_fmt(), false)
    }
    fn xyz_fmt(&self) -> String {
        self.to_string()
    }
    fn xyz_spec_of() -> FieldSpec {
        synthetic(FieldKind::Bool, Vec::new(), None)
    }
}

impl XyzSchema for bool {
    fn xyz_schema() -> Option<Schema> {
        Some(Schema {
            r#type: Some("boolean".to_string()),
            ..Default::default()
        })
    }
}

impl XyzField for String {
    fn xyz_from_value(v: &Value) -> errors::Result<Self> {
        match v {
            Value::String(s) => Ok(s.clone()),
            Value::Number(n) => Ok(n.to_string()), // Go json.Number.String()
            _ => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("expect string, got {}", type_name(v)),
            )),
        }
    }
    fn xyz_zero() -> Self {
        String::new()
    }
    fn xyz_is_zero(&self) -> bool {
        self.is_empty()
    }
    fn xyz_rule_ok(&self, r: &VRule) -> bool {
        rule_ok_bits(r, Some(self.len() as i64), None, self, true)
    }
    fn xyz_fmt(&self) -> String {
        self.clone()
    }
    fn xyz_spec_of() -> FieldSpec {
        synthetic(FieldKind::String, Vec::new(), None)
    }
}

impl XyzSchema for String {
    fn xyz_schema() -> Option<Schema> {
        Some(Schema {
            r#type: Some("string".to_string()),
            ..Default::default()
        })
    }
}

impl XyzField for Duration {
    fn xyz_from_value(v: &Value) -> errors::Result<Self> {
        match v {
            Value::String(s) => {
                parse_duration(s).map_err(|e| errors::Error::new(errors::Kind::InvalidInput, e))
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    let u = u64::try_from(i).map_err(|_| {
                        errors::Error::new(
                            errors::Kind::InvalidInput,
                            "negative duration not supported".to_string(),
                        )
                    })?;
                    return Ok(Duration::from_nanos(u));
                }
                let f = n.as_f64().ok_or_else(|| {
                    errors::Error::new(
                        errors::Kind::InvalidInput,
                        format!("expect duration string, got {}", type_name(v)),
                    )
                })?;
                if f < 0.0 {
                    return Err(errors::Error::new(
                        errors::Kind::InvalidInput,
                        "negative duration not supported".to_string(),
                    ));
                }
                Ok(Duration::from_nanos(f as u64)) // Go：float64 → time.Duration(v) 纳秒
            }
            _ => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("expect duration string, got {}", type_name(v)),
            )),
        }
    }
    fn xyz_zero() -> Self {
        Duration::ZERO
    }
    fn xyz_is_zero(&self) -> bool {
        self.is_zero()
    }
    fn xyz_rule_ok(&self, r: &VRule) -> bool {
        let n = (self.as_secs() as i64)
            .saturating_mul(1_000_000_000)
            .saturating_add(self.subsec_nanos() as i64);
        rule_ok_bits(r, Some(n), Some(n), &self.xyz_fmt(), false)
    }
    fn xyz_fmt(&self) -> String {
        format_duration(*self)
    }
    fn xyz_spec_of() -> FieldSpec {
        synthetic(FieldKind::Duration, Vec::new(), None)
    }
}

impl XyzSchema for Duration {
    fn xyz_schema() -> Option<Schema> {
        Some(Schema {
            r#type: Some("string".to_string()),
            format: Some("duration".to_string()),
            ..Default::default()
        })
    }
}

impl XyzField for DateTime<Utc> {
    fn xyz_from_value(v: &Value) -> errors::Result<Self> {
        match v {
            Value::String(s) => {
                parse_datetime(s).map_err(|e| errors::Error::new(errors::Kind::InvalidInput, e))
            }
            _ => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("expect RFC3339 string, got {}", type_name(v)),
            )),
        }
    }
    fn xyz_zero() -> Self {
        DateTime::UNIX_EPOCH
    }
    fn xyz_is_zero(&self) -> bool {
        *self == DateTime::UNIX_EPOCH
    }
    fn xyz_rule_ok(&self, _r: &VRule) -> bool {
        false // struct kind：无数值/长度语义（Go numericOf 兜底语义不值得复刻）
    }
    fn xyz_fmt(&self) -> String {
        self.to_rfc3339()
    }
    fn xyz_spec_of() -> FieldSpec {
        synthetic(FieldKind::Time, Vec::new(), None)
    }
}

impl XyzSchema for DateTime<Utc> {
    fn xyz_schema() -> Option<Schema> {
        Some(Schema {
            r#type: Some("string".to_string()),
            format: Some("date-time".to_string()),
            ..Default::default()
        })
    }
}

impl XyzField for Vec<u8> {
    fn xyz_from_value(v: &Value) -> errors::Result<Self> {
        match v {
            Value::String(s) => Ok(s.clone().into_bytes()),
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for (i, it) in items.iter().enumerate() {
                    let b = match it {
                        Value::Number(n) => n
                            .as_u64()
                            .and_then(|u| u8::try_from(u).ok())
                            .ok_or_else(|| {
                                errors::Error::new(
                                    errors::Kind::InvalidInput,
                                    format!("index {i}: byte out of range: {it}"),
                                )
                            })?,
                        _ => {
                            return Err(errors::Error::new(
                                errors::Kind::InvalidInput,
                                format!("index {i}: expect number, got {}", type_name(it)),
                            ));
                        }
                    };
                    out.push(b);
                }
                Ok(out)
            }
            _ => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("expect string or byte array, got {}", type_name(v)),
            )),
        }
    }
    fn xyz_zero() -> Self {
        Vec::new()
    }
    fn xyz_is_zero(&self) -> bool {
        self.is_empty()
    }
    fn xyz_rule_ok(&self, r: &VRule) -> bool {
        match r.key.as_str() {
            "min" | "max" | "len" => {
                let n = self.len() as i64;
                rule_ok_bits(r, Some(n), None, &self.xyz_fmt(), false)
            }
            "oneof" => false,
            _ => false,
        }
    }
    fn xyz_fmt(&self) -> String {
        // Go %v []byte 形态："[104 105]"
        let inner = self
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        format!("[{inner}]")
    }
    fn xyz_spec_of() -> FieldSpec {
        synthetic(FieldKind::Bytes, Vec::new(), None)
    }
}

// Vec<T> / Option<T> 自身不实现 XyzField（与 Vec<u8> 的显式实现冲突），
// 宏在字段层按形状选 __field_vec / __field_opt 入口；schema 与 spec 却在
// 形状层需要它们——所以 XyzSchema 与形状构建走独立路径：
impl<T: XyzSchema> XyzSchema for Vec<T> {
    fn xyz_schema() -> Option<Schema> {
        Some(Schema {
            r#type: Some("array".to_string()),
            items: T::xyz_schema().map(Box::new),
            ..Default::default()
        })
    }
}

impl<T: XyzSchema> XyzSchema for Option<T> {
    fn xyz_schema() -> Option<Schema> {
        T::xyz_schema()
    }
}

/// 宏为 `Vec<T>`/`Option<T>` 字段构造元素节点（T 只需 XyzField）。
pub fn spec_of_elem<T: XyzField>() -> FieldSpec {
    T::xyz_spec_of()
}
