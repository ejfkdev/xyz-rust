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
//!     define::<AddArgs, String, _>("user.add", add)
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
//! <app> mcp stdio|sse|http  -> MCP 前端（官方 SDK；--versions 钉定协议版本）
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

pub mod builder;
pub mod builtins;
pub mod cli;
pub mod config;
pub mod ctx;
pub mod dispatch;
pub mod errors;
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
pub mod version;

pub use ctx::Ctx;
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
pub use registry::Registry;
pub use spec::{
    CliFieldHint, CliHints, Entry, FieldMeta, HTTPFieldHint, HTTPHints, MCPFieldHint, MCPHints,
    Schema, XyzArgs, XyzField,
};
pub use version::{set_version, version};
