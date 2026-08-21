// Package cli 是 CLI 前端：消费注册表条目，把它们的 cli 绑定（短名、
// 位置参数、env 回退、接口专属默认值）变成命令树。点分注册名映射成多层
// 子命令："user.add" 变成 "user add"。
//
// 前端只用标准库 + serde（渲染结果时）——不引 clap，输出形态与 Go 版
// 逐字对齐：裸值、对齐 KV、[]struct 表格、--json 翻转成 JSON。

pub mod app;
pub mod completion;
pub mod help;
pub mod parse;
pub mod render;
pub mod tree;

pub use app::{App, ExecContext, Options, bin_name, run, run_context, set_cli_version};
pub use parse::{FlagDef, FlagKind, FlagVal, parse_flags};
pub use render::{render, render_value};

#[cfg(test)]
mod cli_test;
