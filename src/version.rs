// 两个版本槽（spec §11.6/§12.6 的身份分离）：
//
//   - 应用版本：-v/--version 汇报的值 = X-App-Version 响应头 / MCP
//     serverInfo.version（§12.6）。默认为 "dev"（Go 版 Version 变量的
//     对应物），发布前可用 set_version 覆盖——Rust 没有 Go 的
//     -ldflags -X 注入，这是它的等价机制。
//   - SDK 版本：xyz 库自身的版本 = X-XYZ-Version 响应头 /
//     _meta.xyz.sdk_version（§11.6/§12.8），编译期取 crate 版本，永不由
//     应用覆盖。
//
// （cli 前端保留自己的 set_cli_version 以支持 cli::run 直接嵌入。）

use std::sync::OnceLock;

/// xyz 库自身的版本（crate 版本）——与 git tag 同步发布。
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// 返回应用版本串（默认 "dev"）。
pub fn version() -> &'static str {
    VERSION.get_or_init(|| "dev")
}

/// 覆盖应用版本（-v/--version 与 X-App-Version / serverInfo.version）。
/// 必须在派发前调用。
pub fn set_version(v: &'static str) {
    let _ = VERSION.set(v);
}
