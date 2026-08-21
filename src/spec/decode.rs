// decode 是「传输 map → 强类型值」的运行时入口集合，由 #[derive(XyzArgs)]
// 生成的 xyz_decode 按字段形状调用（__field_scalar/__field_opt/__field_vec/
// __field_bytes）。语义与 Go 的 decodeTree + decodeValue 对齐：
// 缺席 → 全局默认 → required 报错 → 零值；null 视同缺席；枚举在转换后
// 按类型等值检查。

use std::cell::Cell;

use serde_json::Value;

use crate::errors;
use crate::spec::field::FieldMeta;
use crate::spec::scalar;
use crate::spec::{JsonMap, XyzField};

/// 递归类型护栏：嵌套 struct 的对象解码每层 +1（Go maxAnalyzeDepth 的
/// 运行时对应物；静态自引用在宏期直接 compile_error）。
const MAX_DECODE_DEPTH: usize = 20;

thread_local! {
    static DECODE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

pub struct DepthGuard;

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DECODE_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

/// 宏生成的 XyzArgs::xyz_from_value（对象分支）先过这里：给出对象 map
/// 并压深。返回的 guard 活在调用栈上，覆盖整段递归解码。
pub fn object_arg(v: &Value) -> errors::Result<(&JsonMap, DepthGuard)> {
    let map = v.as_object().ok_or_else(|| {
        errors::Error::new(
            errors::Kind::InvalidInput,
            format!("expect object, got {}", scalar::type_name(v)),
        )
    })?;
    DECODE_DEPTH.with(|d| {
        let cur = d.get();
        if cur >= MAX_DECODE_DEPTH {
            return Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!(
                    "argument struct nesting exceeds {MAX_DECODE_DEPTH} levels (recursive type?)"
                ),
            ));
        }
        d.set(cur + 1);
        Ok(())
    })?;
    Ok((map, DepthGuard))
}

/// 宏生成的 xyz_decode 开头：元数据长度与字段数一致性（防御宏/运行时
/// 树漂移；debug 断言 + 发布内错误）。
pub fn expect_meta_len(meta: &[FieldMeta], expect: usize) -> errors::Result<()> {
    if meta.len() != expect {
        return Err(errors::Error::new(
            errors::Kind::Internal,
            format!("internal: field meta len {} != {}", meta.len(), expect),
        ));
    }
    Ok(())
}

fn field_with_key<T: XyzField>(fi: &FieldMeta, map: &JsonMap, key: &str) -> errors::Result<T> {
    match map.get(key) {
        Some(v) if !v.is_null() => {
            let t = T::xyz_from_value(v).map_err(|e| field_err(fi, e))?;
            enum_check(fi, &t)?;
            Ok(t)
        }
        _ => match &fi.default {
            Some(d) => T::xyz_from_value(d).map_err(|e| {
                errors::Error::new(
                    e.kind(),
                    format!("field {:?}: invalid default: {e}", fi.json_name),
                )
            }),
            None if fi.required => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("field {:?} is required", fi.json_name),
            )),
            None => Ok(T::xyz_zero()),
        },
    }
}

/// 标量/命名标量/嵌套 struct 字段（按 json 名绑定）。
pub fn field<T: XyzField>(fi: &FieldMeta, map: &JsonMap) -> errors::Result<T> {
    field_with_key(fi, map, fi.json_name.as_str())
}

/// json:"-" 注入字段（env/header 专用）：按 Rust 字段名投递。
pub fn field_skip<T: XyzField>(fi: &FieldMeta, map: &JsonMap) -> errors::Result<T> {
    field_with_key(fi, map, fi.name.as_str())
}

/// Option<T> 字段（Go 指针）：null 视同缺席；有值即 Some。
pub fn field_opt<T: XyzField>(fi: &FieldMeta, map: &JsonMap) -> errors::Result<Option<T>> {
    match map.get(fi.json_name.as_str()) {
        Some(v) if !v.is_null() => {
            let t = T::xyz_from_value(v).map_err(|e| field_err(fi, e))?;
            enum_check(fi, &t)?;
            Ok(Some(t))
        }
        _ => match &fi.default {
            Some(d) => Ok(Some(T::xyz_from_value(d).map_err(|e| {
                errors::Error::new(
                    e.kind(),
                    format!("field {:?}: invalid default: {e}", fi.json_name),
                )
            })?)),
            None if fi.required => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("field {:?} is required", fi.json_name),
            )),
            None => Ok(None),
        },
    }
}

/// Vec<T> 字段：数组逐元素转换；元素 null 落零值（Go decodeValue 同）。
pub fn field_vec<T: XyzField>(fi: &FieldMeta, map: &JsonMap) -> errors::Result<Vec<T>> {
    match map.get(fi.json_name.as_str()) {
        Some(v) if !v.is_null() => {
            let arr = v.as_array().ok_or_else(|| {
                errors::Error::new(
                    errors::Kind::InvalidInput,
                    format!(
                        "field {:?}: expect array, got {}",
                        fi.json_name,
                        scalar::type_name(v)
                    ),
                )
            })?;
            arr.iter()
                .enumerate()
                .map(|(i, it)| {
                    if it.is_null() {
                        return Ok(T::xyz_zero());
                    }
                    T::xyz_from_value(it).map_err(|e| {
                        errors::Error::new(
                            e.kind(),
                            format!("field {:?}: index {i}: {e}", fi.json_name),
                        )
                    })
                })
                .collect()
        }
        _ => match &fi.default {
            Some(d) => match d.as_array() {
                Some(items) => items
                    .iter()
                    .map(|it| T::xyz_from_value(it))
                    .collect::<errors::Result<Vec<_>>>()
                    .map_err(|e| {
                        errors::Error::new(
                            e.kind(),
                            format!("field {:?}: invalid default: {e}", fi.json_name),
                        )
                    }),
                None => Err(errors::Error::new(
                    errors::Kind::InvalidInput,
                    format!("field {:?}: invalid default", fi.json_name),
                )),
            },
            None if fi.required => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("field {:?} is required", fi.json_name),
            )),
            None => Ok(Vec::new()),
        },
    }
}

/// Vec<u8> 字段（Go []byte：字符串或字节数组直接进）。
pub fn field_bytes(fi: &FieldMeta, map: &JsonMap) -> errors::Result<Vec<u8>> {
    field::<Vec<u8>>(fi, map)
}

/// skip 字段上的 Vec<T>（json:"-" 注入：env 值形如数组时仍走数组语义）。
pub fn field_skip_vec<T: XyzField>(fi: &FieldMeta, map: &JsonMap) -> errors::Result<Vec<T>> {
    match map.get(fi.name.as_str()) {
        Some(v) if !v.is_null() => match v.as_array() {
            Some(arr) => arr
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    if it.is_null() {
                        return Ok(T::xyz_zero());
                    }
                    T::xyz_from_value(it).map_err(|e| {
                        errors::Error::new(e.kind(), format!("field {:?}: index {i}: {e}", fi.name))
                    })
                })
                .collect(),
            None => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!(
                    "field {:?}: expect array, got {}",
                    fi.name,
                    scalar::type_name(v)
                ),
            )),
        },
        _ => match &fi.default {
            Some(d) => match d.as_array() {
                Some(items) => items.iter().map(T::xyz_from_value).collect(),
                None => Err(errors::Error::new(
                    errors::Kind::InvalidInput,
                    format!("field {:?}: invalid default", fi.name),
                )),
            },
            None if fi.required => Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!("field {:?} is required", fi.name),
            )),
            None => Ok(Vec::new()),
        },
    }
}

/// 注册期验证 Vec<T> 形状的 hint 默认值（元素逐个可转换）。
pub fn type_check_vec<T: XyzField>(fi: &FieldMeta, v: &Value) -> errors::Result<()> {
    match v {
        Value::Null => Ok(()),
        Value::Array(items) => {
            for it in items {
                if it.is_null() {
                    continue;
                }
                T::xyz_from_value(it).map_err(|e| {
                    errors::Error::new(
                        e.kind(),
                        format!("field {:?}: bad default element {it}: {e}", fi.json_name),
                    )
                })?;
            }
            Ok(())
        }
        Value::String(s) if s.is_empty() => Ok(()),
        other => Err(errors::Error::new(
            errors::Kind::InvalidInput,
            format!(
                "field {:?}: bad default {other}: expect array",
                fi.json_name
            ),
        )),
    }
}

/// 注册期验证 Option<T> 形状的 hint 默认值。
pub fn type_check_opt<T: XyzField>(fi: &FieldMeta, v: &Value) -> errors::Result<()> {
    if v.is_null() {
        return Ok(());
    }
    T::xyz_from_value(v).map(|_| ()).map_err(|e| {
        errors::Error::new(
            e.kind(),
            format!("field {:?}: bad default {v}: {e}", fi.json_name),
        )
    })
}

fn field_err(fi: &FieldMeta, e: errors::Error) -> errors::Error {
    errors::Error::new(e.kind(), format!("field {:?}: {e}", fi.json_name))
}

fn enum_check<T: XyzField>(fi: &FieldMeta, t: &T) -> errors::Result<()> {
    if fi.enum_values.is_empty() {
        return Ok(());
    }
    // 值等价经 fmt 形态比较（枚举值与目标值同源转换，位级一致）。
    let fmt = t.xyz_fmt();
    let hit = fi
        .enum_values
        .iter()
        .filter_map(|e| T::xyz_from_value(e).ok())
        .any(|cand| cand.xyz_fmt() == fmt);
    if !hit {
        return Err(errors::Error::new(
            errors::Kind::InvalidInput,
            format!(
                "field {:?}: value must be one of [{}]",
                fi.json_name,
                fi.enum_values
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ));
    }
    Ok(())
}

/// 宏生成的 xyz_type_check 逐索引派发到这里：注册期验证 hint 默认值可被
/// 目标字段转换（Go normalizeHintDefault 的对应物）。
pub fn type_check_of<T: XyzField>(fi: &FieldMeta, v: &Value) -> errors::Result<()> {
    T::xyz_from_value(v).map(|_| ()).map_err(|e| {
        errors::Error::new(
            e.kind(),
            format!("field {:?}: bad default {v}: {e}", fi.json_name),
        )
    })
}
