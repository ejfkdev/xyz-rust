// spec 是一条命令的唯一事实来源：一个 #[derive(XyzArgs)] 的入参 struct
// 在注册期被分析一次，产出所有前端需要的元数据（CLI flag、HTTP 绑定、
// MCP JSON Schema）外加一条 Invoke 管线——把传输形态的入参
// （JSON map）解码成强类型 struct、补默认、校验、执行 handler。
//
// Rust 没有运行时反射，Go 侧 reflect 承担的事由三件套替代：
//   - 过程宏 xyz_rust_macros::XyzArgs 生成静态字段描述树与解码/校验代码；
//   - XyzField 提供标量转换（无损检查）、零值、单规则校验（对齐反射的
//     scalarValue/isZero/numericOf）；
//   - XyzSchema 提供结果类型的 JSON Schema（对齐 buildOutputSchema）。
//
// 前端从不接触命令的类型参数：它们消费 Entry 与其元数据，因此新增
// 传输无需改动核心。每个前端实现的契约是「把传输的输入归约成
// serde_json Map，调用 Entry.invoke，渲染返回的 Value 或分类错误」。

use crate::errors;
use serde_json::{Map, Value};

pub mod command;
pub mod decode;
pub mod entry;
pub mod field;
pub mod scalar;
pub mod schema;
#[cfg(test)]
mod spec_test;
pub mod validate;

pub use command::{CliFieldHint, CliHints, HTTPFieldHint, HTTPHints, MCPFieldHint, MCPHints};
pub use entry::Entry;
pub use field::{FieldKind, FieldMeta, FieldSpec, HTTPField, MCPField};
pub use scalar::{format_duration as fmt_duration, parse_duration};
pub use schema::Schema;

pub type JsonMap = Map<String, Value>;

pub type HResult<T> = errors::Result<T>;

/// XyzField 是「可作 command 入参的叶子类型」的契约：标量、String、
/// Duration、DateTime<Utc>、Vec<T>/Option<T>（T: XyzField），以及由
/// #[derive(XyzField)] 生成的命名标量 newtype 与 #[derive(XyzArgs)] 的
/// 嵌套 struct（后者经由 from_value 递归进自己的 xyz_decode）。
pub trait XyzField: Sized + 'static {
    /// 把任意传输来的 Value 转换成 Self；数值转换带无损检查
    /// （"3.7" 永不静默变成整数）。
    fn xyz_from_value(v: &Value) -> errors::Result<Self>;

    /// 校验语境下的零值（对齐 go-playground/validator 的 required）。
    fn xyz_zero() -> Self;
    fn xyz_is_zero(&self) -> bool;

    /// 单条校验规则的判定（min/max/len/gt/gte/lt/lte/oneof/email）。
    fn xyz_rule_ok(&self, r: &validate::VRule) -> bool;

    /// oneof 比较与 %v 显示用的字符串形态。
    fn xyz_fmt(&self) -> String;

    /// 本类型作为字段/元素的静态形状节点（kind + 嵌套 children）。
    /// 标量实现给出自身 kind 的空 children 节点；XyzArgs 派生给出的
    /// 是 kind=Struct 且 children=xyz_spec() 的节点。
    fn xyz_spec_of() -> FieldSpec;

    /// 嵌套校验递归：struct 元素把自己的 children 元数据交回来校验；
    /// 标量默认为空实现。宏生成代码对 Vec<T>/Option<T> 元素统一调用，
    /// 使类型别名（无 XyzArgs）与嵌套 struct 共用一条通路。
    fn xyz_validate_elem(&self, _meta: &FieldMeta) -> errors::Result<()> {
        Ok(())
    }
}

/// XyzSchema 描述一个结果类型的 JSON Schema（MCP tool.outputSchema /
/// OpenAPI 响应 schema）。无法 schematize 的实现返回 None（Go 侧
/// buildOutputSchema 对接口/map 等返回错误 → nil 的等价物）。
pub trait XyzSchema: Sized {
    fn xyz_schema() -> Option<Schema>;
}

/// XyzArgs 是入参 struct 的契约，全部由 #[derive(XyzArgs)] 生成（宏标签
/// 词汇表见 crate 文档）：静态描述树、惰性解析的运行时元数据、解码与
/// 校验。
pub trait XyzArgs: XyzField + 'static {
    /// 静态字段描述（tag 原串 + 类型形状），宏按声明序生成。
    fn xyz_spec() -> Vec<FieldSpec>;

    /// 解析后的运行时元数据（tag 解析 / 默认值 / 枚举），OnceLock 惰性
    /// 缓存；解析失败在注册期报告（与注册期即报错原则一致）。
    fn xyz_meta() -> errors::Result<&'static [FieldMeta]>;

    /// 把传输 map 解码成 Self：补全局默认、校验 required、枚举检查。
    /// meta 与字段按索引对齐（xyz_meta 的产出）。
    fn xyz_decode(map: &JsonMap, meta: &[FieldMeta]) -> errors::Result<Self>;

    /// 在已解码值上跑 validate 规则树（递归嵌套）。
    fn xyz_validate(&self, meta: &[FieldMeta]) -> errors::Result<()>;

    /// 注册期验证一个 Define-time hint 默认值能被目标字段接受
    /// （Go normalizeHintDefault 的对应物）。宏按索引派发到字段类型。
    fn xyz_type_check(idx: usize, meta: &[FieldMeta], v: &Value) -> errors::Result<()>;
}

/// 供宏生成代码构筑元素节点：Vec<Port> / Option<Addr> 里的 Port/Addr
/// 形状来自各自的 XyzField::xyz_spec_of()。
pub fn spec_of<T: XyzField>() -> FieldSpec {
    T::xyz_spec_of()
}
