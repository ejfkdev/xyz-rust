// Version 是 -v/--version 处理（见 dispatch）汇报的版本号。默认取
// CARGO_PKG_VERSION（即 crate 自身的发布版本），发布前可用 set_version
// 覆盖——Rust 没有 Go 的 -ldflags -X 注入，这是它的等价机制。
// （cli 前端保留自己的 Version 以支持 cli::run 直接嵌入。）

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// 返回当前版本串（默认 crate 版本）。
pub fn version() -> &'static str {
    VERSION.get_or_init(|| env!("CARGO_PKG_VERSION"))
}

/// 覆盖 -v/--version 输出的版本串。必须在派发前调用。
pub fn set_version(v: &'static str) {
    let _ = VERSION.set(v);
}
