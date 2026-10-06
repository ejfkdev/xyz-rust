//! # xyz-rust — One definition, three interfaces
//!
//! 一次定义（入参 struct + 校验 + 每渠道细节），一个二进制自动讲三种接口：
//! **CLI 子命令**、**HTTP REST 服务**（带 OpenAPI 文档）与 **MCP 工具服务器**
//! （官方 Rust SDK `rmcp`）。运行模式由库自行判断。
//!
//! ```no_run
//! use xyz_rust::{define, CliHints, HTTPHints, XyzArgs};
//! use xyz_rust::errs;
//!
//! #[derive(XyzArgs)]
//! struct AddArgs {
//!     #[xyz(desc = "用户名", required, cli = "positional", http = "path")]
//!     name: String,
//!     #[xyz(desc = "年龄", default = "18")]
//!     age: i32,
//! }
//!
//! fn add(_ctx: &xyz_rust::Ctx, in_: &AddArgs) -> errs::Result<String> {
//!     Ok(format!("{} is {}", in_.name, in_.age))
//! }
//!
//! fn main() {
//!     define::<AddArgs, String, _, _>("user.add", add)
//!         .summary("添加用户")
//!         .cli(CliHints { usage: "add <name>".into(), ..Default::default() })
//!         .http(HTTPHints { method: "POST".into(), path: "/users/{name}".into(), ..Default::default() })
//!         .run(); // 注册 + 派发 + exit — 整个程序就这一条链
//! }
//! ```
//!
//! 进程级默认注册表背后的派生器：`Main` 派发默认注册表并内部调用
//! `std::process::exit`，因此 main 里写的清理代码不会在它们之后执行。需要
//! defer 清理、自定义退出码、多个注册表或嵌入派发器时，用显式注册表的
//! `run` / `run_config` 版本，它们只返回退出码。
//!
//! 没有任何注册命令的注册表是静默 no-op：派发器直接退出 0，什么都不打印。
//!
//! 模式探测：
//!
//! ```text
//! <app> [命令] ...          -> CLI 前端（子命令、flag、位置参数、-h / -v）
//! <app> mcp stdio|http       -> MCP 前端（官方 SDK；--versions 钉定协议版本；sse 随
//!                                 2026-07-28 修订从官方 Rust SDK 移除，报错退出）
//! <app> serve [--addr ...]  -> HTTP 前端（REST + /openapi.json + /mcp）
//! <app> （无参数）| help    -> 总览（列出三种形态与命令表）
//! ```
//!
//! 模式关键词默认为 serve / mcp / help 且是保留的顶层名字；两者都可经
//! `Config.modes` 重命名。派发在 [`dispatch`] 模块，配置类型在
//! [`config`]，内置参数解析在 [`builtins`]，总览渲染在 [`overview`]，流式
//! 构建器在 [`builder`]。

// 自别名：宏生成的 ::xyz_rust:: 绝对路径在库自身测试里同样可达。
extern crate self as xyz_rust;

#[cfg(test)]
mod dispatch_test;

pub mod blocks;
pub mod builder;
pub mod builtins;
pub mod cli;
pub mod config;
pub mod ctx;
pub mod dispatch;
pub mod errors;
pub mod lang;
pub mod logx;
// httpapi 在 http 或 mcp 任一通道存在时都在树中（Go 的结构同款：mcp 复用
// http 中间件积木；两个通道都裁掉时整体消失）。
#[cfg(any(feature = "http", feature = "mcp"))]
pub mod httpapi;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod overview;
pub mod registry;
pub mod spec;
pub mod termx;
pub mod version;

pub use ctx::Ctx;

/// 当前界面语言（spec §14 item7）：进程级语言槽（`--xyz.lang` > Config >
/// 环境检测 > en，§15.5）。
pub fn language() -> &'static str {
    crate::lang::current().as_str()
}

/// 进程主输出是否交互式（TTY）。这是格式轴（§10.7）与保留的样式轴
/// （§10.7a）共享的同一探针接缝。
pub fn interactive() -> bool {
    crate::termx::interactive()
}

/// 颜色是否被环境抑制（`NO_COLOR` 非空或 `TERM=dumb`；§10.7a 保留样式轴
/// 的输入，先以访问器形式暴露）。
pub fn no_color() -> bool {
    crate::termx::no_color()
}

/// 环境上下文快照（现象 §14 item7 的 `Env()` 访问器）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvContext {
    pub language: &'static str,
    pub interactive: bool,
    pub no_color: bool,
}

/// 一次取齐语言/交互式/颜色抑制（spec §14 item7）。
pub fn env() -> EnvContext {
    EnvContext {
        language: language(),
        interactive: interactive(),
        no_color: no_color(),
    }
}

/// 请求上下文携带的界面语言（spec §11.7）；未绑定时回退进程语言
/// （Go `xyz.LanguageFromCtx(ctx)` 的对应物）。
pub fn language_from_ctx(ctx: &Ctx) -> String {
    ctx.language()
        .map(|s| s.to_string())
        .unwrap_or_else(|| crate::lang::current().as_str().to_string())
}
pub use errors as errs;

// 派生宏在最上层以惯用名导出（xyz_rust::XyzArgs 直接可用作 #[derive]）。
pub use xyz_rust_macros::{XyzArgs, XyzField, XyzOutput};

// 宏生成代码用的绝对路径词汇表（用户 crate 只依赖 xyz-rust）。
pub use chrono;
pub use serde;
pub use serde_json;

pub use builder::{Builder, Definable, define};
pub use config::{Capabilities, Config, ModeWords};
pub use dispatch::{main as main_entry, main_config, run, run_config};
pub use errors::{Error, Kind};
pub use lang::{XyzLang, set as set_lang, t, tf};
pub use registry::Registry;
pub use spec::{
    CliFieldHint, CliHints, Entry, FieldMeta, HTTPFieldHint, HTTPHints, MCPFieldHint, MCPHints,
    Schema, XyzArgs, XyzField,
};
pub use version::{set_version, version};
