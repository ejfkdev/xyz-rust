// CLI 前端：注册表 → 命令树 → flag 解析 → Invoke + 渲染。
//
// 输出契约：命令结果去 stdout；错误与诊断去 stderr。--json 生效时结果
// 以两空格缩进 JSON 输出；否则用人类可读渲染（render.rs）。
//
// 与 Go 版差异：进程名取 std::env::args()[0] 的 basename（Go filepath.Base
// 同）；输出流用 Arc<Mutex<_>> 共享（Go 的 io.Writer 接口引用语义在
// Rust 里需要锁来获得 &mut 写出权）。

use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::cli::completion::print_completion;
use crate::ctx::Ctx;
use crate::errors;
use crate::registry::Registry;
use crate::spec::Entry;

use super::tree::{CmdNode, build_tree};

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// CLI 前端 -v/--version 汇报的版本（cli::run 直接嵌入时使用；根派发器
/// 在到达这里之前用 version 模块应答 -v）。
static CLI_VERSION: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// 设置 CLI 前端 -v 输出的版本串。
pub fn set_cli_version(v: &'static str) {
    let _ = CLI_VERSION.set(v);
}

fn cli_version() -> &'static str {
    CLI_VERSION.get_or_init(crate::version::version)
}

pub struct App {
    pub(crate) root: CmdNode,
    pub(crate) out: SharedWriter,
    pub(crate) err_out: SharedWriter,
    pub(crate) mws: Vec<ExecFunc>,
}

/// 嵌入场景的前端级配置（零值保持 stdout/stderr）。
#[derive(Default)]
pub struct Options {
    pub out: Option<Box<dyn Write + Send>>,
    pub err_out: Option<Box<dyn Write + Send>>,
}

/// 一次叶子命令执行的只读快照，交给 Execute 中间件（use_mw）。
pub struct ExecContext {
    /// 点分注册名，如 user.add。
    pub path: String,
    /// 命令元数据（Hints、InputSchema、OutputSchema）。
    pub entry: Arc<Entry>,
    /// --json 是否生效（未调 next 自行渲染时可参考）。
    pub json: bool,
    /// 结果的输出目标（Arc<Mutex>，lock 后即 &mut dyn Write）。
    pub out: SharedWriter,
}

/// Execute 中间件：args 是已解析的入参 map（flag、env 与位置参数已应用）；
/// next(&mut args) 续链到 Invoke + 渲染（可多次调用）。返回值语义与命令
/// 错误一致（按分类映射退出码）。
pub type ExecFunc = Box<
    dyn Fn(
            &Ctx,
            &ExecContext,
            &mut Map<String, Value>,
            &mut dyn FnMut(&mut Map<String, Value>) -> errors::Result<()>,
        ) -> errors::Result<()>
        + Send
        + Sync,
>;

impl App {
    /// 由注册表构建命令树。不可绑定的字段形状（嵌套 struct、元素为
    /// struct 的切片）与有歧义的位置参数（optional 后面的 required）是
    /// 配置错误。
    pub fn new(reg: &Registry) -> errors::Result<App> {
        let root = build_tree(reg)?;
        Ok(App {
            root,
            out: Arc::new(Mutex::new(Box::new(std::io::stdout()))),
            err_out: Arc::new(Mutex::new(Box::new(std::io::stderr()))),
            mws: Vec::new(),
        })
    }

    /// 带前端选项的构建；None 选项保持默认。
    pub fn new_with_options(reg: &Registry, opts: Options) -> errors::Result<App> {
        let mut a = App::new(reg)?;
        if let Some(o) = opts.out {
            a.out = Arc::new(Mutex::new(o));
        }
        if let Some(e) = opts.err_out {
            a.err_out = Arc::new(Mutex::new(e));
        }
        Ok(a)
    }

    /// 重定向输出流；None 保持现状。嵌入大程序/测试必备。
    pub fn set_output(
        &mut self,
        out: Option<Box<dyn Write + Send>>,
        err_out: Option<Box<dyn Write + Send>>,
    ) {
        if let Some(o) = out {
            self.out = Arc::new(Mutex::new(o));
        }
        if let Some(e) = err_out {
            self.err_out = Arc::new(Mutex::new(e));
        }
    }

    /// 追加 Execute 中间件（最外层最先）。next() 续链到 Invoke + 渲染；
    /// 中间件可改写入参、短路（跳过 next 自绘）或包装 next 计时。
    pub fn use_mw(&mut self, mw: ExecFunc) {
        self.mws.push(mw);
    }

    /// App 自身的执行入口。
    pub fn run(&mut self, args: &[String]) -> i32 {
        self.run_ctx(&Ctx::new(), args)
    }

    pub fn run_ctx(&mut self, ctx: &Ctx, args: &[String]) -> i32 {
        // 内建 completion 子命令：生成 shell 补全脚本（bash/zsh/fish）。
        if args.first().map(String::as_str) == Some("completion") {
            let shell = args.get(1).map(String::as_str).unwrap_or("bash");
            return print_completion(
                &self.root_collect_top(),
                &mut *self.out.lock().unwrap(),
                &mut *self.err_out.lock().unwrap(),
                &bin_name(),
                shell,
            );
        }
        let bin = bin_name();
        for arg in args {
            if arg == "-v" || arg == "--version" {
                let _ = writeln!(
                    self.out.lock().unwrap(),
                    "{} version {}",
                    bin,
                    cli_version()
                );
                return 0;
            }
        }
        let mut json_out = false;
        let mut filtered: Vec<String> = Vec::with_capacity(args.len());
        for arg in args {
            if arg == "--json" {
                json_out = true;
                continue;
            }
            filtered.push(arg.clone());
        }
        if let Err(e) = self.execute(ctx, self.root.clone(), &filtered, json_out, &bin) {
            let _ = writeln!(self.err_out.lock().unwrap(), "{e}");
            return exit_code_of(&e);
        }
        0
    }

    fn root_collect_top(&self) -> Vec<String> {
        self.root
            .children
            .iter()
            .map(|c| c.segment.clone())
            .collect()
    }

    pub fn execute(
        &mut self,
        ctx: &Ctx,
        mut node: CmdNode,
        args: &[String],
        json_out: bool,
        bin: &str,
    ) -> errors::Result<()> {
        let mut rest = args;
        // 逐段下沉子命令树（别名与子命令段等价）
        while let Some(first) = rest.first() {
            let hit = node
                .children
                .iter()
                .find(|c| c.segment == *first || (c.leaf && c.aliases.iter().any(|a| a == first)));
            match hit {
                Some(child) => {
                    node = child.clone();
                    rest = &rest[1..];
                }
                None => break,
            }
        }
        for t in rest {
            if t == "-h" || t == "--help" {
                return self.print_help(&node, bin);
            }
        }
        if !node.leaf {
            return self.print_help(&node, bin);
        }
        let (fvals, pos) = super::parse::parse_flags(&node.defs, rest)?;
        if pos.len() < node.min_pos || pos.len() > node.max_pos {
            return Err(errors::Error::new(
                errors::Kind::InvalidInput,
                format!(
                    "{}: 位置参数数量不符（需要 {} 到 {} 个，收到 {} 个）",
                    node.path.replace('.', " "),
                    node.min_pos,
                    node.max_pos,
                    pos.len()
                ),
            ));
        }
        let mut m = Map::new();
        for (i, d) in node.defs.iter().enumerate() {
            let fv = &fvals[i];
            if fv.seen {
                match d.kind {
                    super::parse::FlagKind::Bool => {
                        m.insert(d.field.json_name.clone(), Value::Bool(fv.boolean));
                    }
                    super::parse::FlagKind::Slice => {
                        m.insert(
                            d.field.json_name.clone(),
                            Value::Array(
                                fv.list.iter().map(|s| Value::String(s.clone())).collect(),
                            ),
                        );
                    }
                    _ => {
                        m.insert(d.field.json_name.clone(), Value::String(fv.str.clone()));
                    }
                }
                continue;
            }
            if let Some(env) = &d.field.cli.env_var
                && let Ok(v) = std::env::var(env)
                && !v.is_empty()
            {
                m.insert(d.field.json_name.clone(), Value::String(v));
                continue;
            }
            if let Some(def) = &d.field.cli.default {
                m.insert(d.field.json_name.clone(), def.clone());
            }
        }
        // json:"-"（#[xyz(skip)]）的注入字段：env 值以 Rust 字段名为键送达。
        for f in &node.env_only {
            if let Some(env) = &f.cli.env_var
                && let Ok(v) = std::env::var(env)
                && !v.is_empty()
            {
                m.insert(f.name.clone(), Value::String(v));
            }
        }
        for (i, f) in node.pos_f.iter().enumerate() {
            if i < pos.len() {
                m.insert(f.json_name.clone(), Value::String(pos[i].clone()));
            }
        }
        let entry = node
            .entry
            .as_ref()
            .ok_or_else(|| {
                errors::Error::new(errors::Kind::Internal, "leaf without entry".to_string())
            })?
            .clone();
        let ec = ExecContext {
            path: node.path.clone(),
            entry,
            json: json_out,
            out: Arc::clone(&self.out),
        };

        // 中间件洋葱链：自内向外构建（最晚注册的最外层）。
        let terminal = |ctx: &Ctx, ec: &ExecContext, args: &mut Map<String, Value>| {
            let out = (ec.entry.invoke)(ctx, args)?;
            let mut w = ec.out.lock().unwrap();
            if ec.json {
                let s = serde_json::to_string_pretty(&out).map_err(|e| {
                    errors::Error::new(errors::Kind::Internal, format!("result serialization: {e}"))
                })?;
                writeln!(*w, "{s}").map_err(io_err)?;
            } else {
                super::render::render(&mut **w, &out)?;
            }
            Ok(())
        };
        // 中间件洋葱链：入参 map 经 next 的 &mut 参数逐层下传（Go 接口引用
        // 语义的 Rust 等价物），每层用自己的 RefCell 槽位保存内层链，支撑
        // next 的多次调用。
        let ec_ref = &ec;
        #[allow(clippy::type_complexity)]
        let mut chain: Box<dyn FnMut(&mut Map<String, Value>) -> errors::Result<()> + '_> =
            Box::new(|m| terminal(ctx, ec_ref, m));
        for i in (0..self.mws.len()).rev() {
            let mw = &self.mws[i];
            let inner = chain;
            let cell = std::cell::RefCell::new(Some(inner));
            chain = Box::new(move |m: &mut Map<String, Value>| {
                let mut guard = cell.borrow_mut();
                let mut inner_fn = guard.take().unwrap();
                let res = {
                    let mut next_local = |mm: &mut Map<String, Value>| inner_fn(mm);
                    let mut next_trait: &mut dyn FnMut(
                        &mut Map<String, Value>,
                    ) -> errors::Result<()> = &mut next_local;
                    (mw)(ctx, ec_ref, m, &mut next_trait)
                };
                *guard = Some(inner_fn);
                res
            });
        }
        let mut m = m;
        chain(&mut m)
    }

    pub(crate) fn print_help(&mut self, node: &CmdNode, bin: &str) -> errors::Result<()> {
        super::help::print_help(&mut *self.out.lock().unwrap(), node, bin)
    }
}

fn io_err(e: std::io::Error) -> errors::Error {
    errors::Error::new(errors::Kind::Internal, format!("write error: {e}"))
}

/// 把 handler 错误映射成退出码：有分类的按表；未分类（flag/用法等）给 2。
pub(crate) fn exit_code_of(e: &errors::Error) -> i32 {
    match errors::classify(e) {
        Some(kind) => errors::exit_code(kind),
        None => 2,
    }
}

/// 进程名：std::env::args()[0] 的 basename；异常值兜底 "app"。
pub fn bin_name() -> String {
    let arg0 = std::env::args().next().unwrap_or_default();
    let base = std::path::Path::new(&arg0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("app");
    if base.is_empty() || base == "." || base == "/" {
        "app".to_string()
    } else {
        base.to_string()
    }
}

/// 一次调用形态：构建 + 执行。
pub fn run(reg: &Registry, args: &[String]) -> i32 {
    run_context(&Ctx::new(), reg, args, Options::default())
}

/// 带上下文的执行（取消信号流进被调 handler）。
pub fn run_context(ctx: &Ctx, reg: &Registry, args: &[String], opts: Options) -> i32 {
    let mut a = match App::new_with_options(reg, opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    a.run_ctx(ctx, args)
}
