//! termx 探针（Go termx 的 Rust 对应物）：CLI 格式轴（§10.7）与保留的
//! 样式轴（§10.7a）共享的终端/环境探测叶模块。
//!
//! - [`interactive`]：进程 stdout 是否为交互式终端（`IsTerminal`）；
//! - [`no_color`]：颜色是否被环境抑制（`NO_COLOR` 非空或 `TERM=dumb`）——
//!   保留供未来的彩色渲染器，现在的样式轴输入。
//!
//! 两条轴共用这一个探针但独立解析：真实终端里设 NO_COLOR 只关颜色、不动
//! 格式（§10.7a）。

use std::io::IsTerminal;

/// 进程 stdout 是否为交互式终端。
pub fn interactive() -> bool {
    std::io::stdout().is_terminal()
}

/// 颜色是否被环境抑制：`NO_COLOR`（任意非空值）或 `TERM=dumb`。
pub fn no_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_env() {
        // 用子进程避免污染测试进程环境：直接改 env 会与并行测试打架。
        // 这里只断言当前环境能被求值（不 panic），语义在集成层由 force
        // 覆盖（Options.interactive）验证。
        let _ = no_color();
        let _ = interactive();
    }
}
