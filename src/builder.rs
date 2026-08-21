// builder 是流式主入口：一条 Define 链配置整个程序。define 打开它，
// summary/description/cli/http/mcp 配置当前命令，also 追加已构建完成的
// 命令，终结的 run 把一切注册进默认注册表、派发进程参数并按结果退出。
//
// Rust 的形态：首条命令内联配置，后续命令以完整 define(...) 链交给
// also（&[&dyn Definable] 切片，异构经由 trait object 擦除）。

use serde::Serialize;

use crate::config::Config;
use crate::ctx::Ctx;
use crate::errors;
use crate::registry::Registry;
use crate::spec::command::Command;
use crate::spec::{XyzArgs, XyzSchema};
use crate::{dispatch, spec};

/// Definable 由任何构建完成的命令定义实现（spec::Command 或 builder 返回
/// 的 Builder），使异构命令能并入一条链或一次 main 调用。
pub trait Definable {
    fn register(&self, reg: &Registry) -> errors::Result<()>;
}

impl<T, R, F, E> Definable for Command<T, R, F, E>
where
    T: XyzArgs,
    R: XyzSchema + Serialize,
    F: Fn(&Ctx, &T) -> std::result::Result<R, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn register(&self, reg: &Registry) -> errors::Result<()> {
        self.entry().map(|e| {
            let _ = reg.add(std::sync::Arc::new(e));
        })
    }
}

/// Builder 是一条定义链：当前命令内联配置，命令列表经 also 追加，run
/// 注册 + 派发 + 退出。
pub struct Builder<T, R, F, E>
where
    T: XyzArgs,
    R: XyzSchema + Serialize,
    F: Fn(&Ctx, &T) -> std::result::Result<R, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    cmd: Command<T, R, F, E>,
    reg: &'static Registry,
    committed: bool,
    config: Config,
    err: Option<errors::Error>,
}

/// 在默认注册表上打开一条定义链。
pub fn define<T, R, F, E>(name: &str, h: F) -> Builder<T, R, F, E>
where
    T: XyzArgs,
    R: XyzSchema + Serialize,
    F: Fn(&Ctx, &T) -> std::result::Result<R, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    Builder {
        cmd: Command::new(name, h),
        reg: Registry::default(),
        committed: false,
        config: Config::default(),
        err: None,
    }
}

impl<T, R, F, E> Builder<T, R, F, E>
where
    T: XyzArgs,
    R: XyzSchema + Serialize,
    F: Fn(&Ctx, &T) -> std::result::Result<R, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    /// 单行描述。
    pub fn summary(mut self, s: impl Into<String>) -> Self {
        self.cmd = self.cmd.summary(s);
        self
    }

    /// 长描述。
    pub fn description(mut self, s: impl Into<String>) -> Self {
        self.cmd = self.cmd.description(s);
        self
    }

    /// 命令级 CLI 配置。
    pub fn cli(mut self, h: spec::CliHints) -> Self {
        self.cmd = self.cmd.cli(h);
        self
    }

    /// 命令级 HTTP 配置。
    pub fn http(mut self, h: spec::HTTPHints) -> Self {
        self.cmd = self.cmd.http(h);
        self
    }

    /// 命令级 MCP 配置。
    pub fn mcp(mut self, h: spec::MCPHints) -> Self {
        self.cmd = self.cmd.mcp(h);
        self
    }

    /// 设置 Run/RunArgs 使用的派发器配置（模式词、通道能力）。在链任意
    /// 位置调用；run_args_config 为那次调用显式指定 Config。
    pub fn configure(mut self, cfg: Config) -> Self {
        self.config = cfg;
        self
    }

    /// 把当前命令与传入的每条命令都注册进同一默认注册表，然后续链。可
    /// 再次调用追加更多。注册失败终止链：在 run 处浮出。
    pub fn also(mut self, cmds: &[&dyn Definable]) -> Self {
        if self.err.is_none() && !self.committed {
            match self.commit_current() {
                Ok(()) => self.committed = true,
                Err(e) => self.err = Some(e),
            }
        }
        if self.err.is_none() {
            for c in cmds {
                if let Err(e) = c.register(self.reg) {
                    self.err = Some(e);
                    break;
                }
            }
        }
        self
    }

    fn commit_current(&self) -> errors::Result<()> {
        self.cmd.entry().map(|entry| {
            let _ = self.reg.add(std::sync::Arc::new(entry));
        })
    }

    /// 注册（若尚未注册）、按进程参数派发默认注册表、按结果退出。其后
    /// 的代码按设计不可达。
    pub fn run(self) -> ! {
        let args: Vec<String> = std::env::args().skip(1).collect();
        std::process::exit(self.run_args(args));
    }

    /// 带自定义配置的 run。
    pub fn run_config(self, cfg: Config) -> ! {
        let args: Vec<String> = std::env::args().skip(1).collect();
        std::process::exit(self.run_args_config(args, cfg));
    }

    /// run 的可测试/可嵌入形态：注册、派发并返回退出码，不退出进程。
    /// 使用链上 configure 的配置（零值 = 全默认）。
    pub fn run_args(mut self, args: Vec<String>) -> i32 {
        let cfg = std::mem::take(&mut self.config);
        self.run_args_config(args, cfg)
    }

    /// 带自定义配置的 run_args。
    pub fn run_args_config(mut self, args: Vec<String>, cfg: Config) -> i32 {
        if self.err.is_none() && !self.committed {
            match self.commit_current() {
                Ok(()) => self.committed = true,
                Err(e) => self.err = Some(e),
            }
        }
        if let Some(e) = self.err {
            eprintln!("{e}");
            return 2;
        }
        dispatch::run_config(self.reg, args, cfg)
    }
}

impl<T, R, F, E> Definable for Builder<T, R, F, E>
where
    T: XyzArgs,
    R: XyzSchema + Serialize,
    F: Fn(&Ctx, &T) -> std::result::Result<R, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn register(&self, reg: &Registry) -> errors::Result<()> {
        self.cmd.entry().map(|entry| {
            let _ = reg.add(std::sync::Arc::new(entry));
        })
    }
}
