// logx 是库自身的诊断出口：写给 stderr 的分级日志（xyz[level]: 前缀）。
// 默认级别 Info；根派发器读取 --xyz.log-level（代码侧：logx::set_level）。
//
// 库只在这里写诊断——命令结果与用法错误走各自输出流，不受日志级别影响。

use std::sync::atomic::{AtomicI32, Ordering};

/// Level 是冗余级别。数值越大越安静。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
#[derive(Default)]
pub enum Level {
    /// 排在 0 位：Config.log_level 的零值，语义为「保持默认」。
    #[default]
    Unset = 0,
    /// 派发/协商追踪（调试 MCP 时最有用）。
    Debug = 1,
    /// 默认级别：启动通知、所选的模式。
    Info = 2,
    /// 只留警告（被禁用的通道、未受保护的传输）。
    Warn = 3,
    /// 只留错误诊断。
    Error = 4,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Unset => "info",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

static CURRENT: AtomicI32 = AtomicI32::new(Level::Info as i32);

/// 设置进程级冗余级别；Unset 或越界值保持现状。
pub fn set_level(l: Level) {
    match l {
        Level::Debug | Level::Info | Level::Warn | Level::Error => {
            CURRENT.store(l as i32, Ordering::Relaxed);
        }
        Level::Unset => {}
    }
}

/// 报告 l 级别的消息是否会被打印。
pub fn enabled(l: Level) -> bool {
    (l as i32) >= CURRENT.load(Ordering::Relaxed)
}

/// 把 flag 值（debug|info|warn|error）映射成 Level。
pub fn parse_level(s: &str) -> Result<Level, crate::errors::Error> {
    match s.trim().to_ascii_lowercase().as_str() {
        "debug" => Ok(Level::Debug),
        "info" => Ok(Level::Info),
        "warn" | "warning" => Ok(Level::Warn),
        "error" => Ok(Level::Error),
        _ => Err(crate::errors::Error::new(
            crate::errors::Kind::Internal,
            format!("unknown log level {s:?} (want debug|info|warn|error)"),
        )),
    }
}

fn emit(l: Level, msg: std::fmt::Arguments<'_>) {
    if !enabled(l) {
        return;
    }
    eprintln!("xyz[{}]: {}", l.as_str(), msg);
}

pub fn debugf(args: std::fmt::Arguments<'_>) {
    emit(Level::Debug, args);
}

pub fn infof(args: std::fmt::Arguments<'_>) {
    emit(Level::Info, args);
}

pub fn warnf(args: std::fmt::Arguments<'_>) {
    emit(Level::Warn, args);
}

pub fn errorf(args: std::fmt::Arguments<'_>) {
    emit(Level::Error, args);
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::logx::debugf(format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logx::infof(format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logx::warnf(format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::logx::errorf(format_args!($($arg)*)) };
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_level_forms() {
        assert_eq!(parse_level("debug").unwrap(), Level::Debug);
        assert_eq!(parse_level("INFO").unwrap(), Level::Info);
        assert_eq!(parse_level("warning").unwrap(), Level::Warn);
        assert_eq!(parse_level("error").unwrap(), Level::Error);
        assert!(parse_level("verbose").is_err());
        assert!(parse_level("").is_err());
    }

    #[test]
    fn set_level_and_enabled() {
        set_level(Level::Error);
        assert!(!enabled(Level::Info));
        assert!(enabled(Level::Error));
        set_level(Level::Debug);
        assert!(enabled(Level::Info));
    }
}
