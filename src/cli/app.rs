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

use super::format::Format;
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
    /// 代码级全局默认格式（spec §10.7 第四层）：""|auto 按 TTY 解析。
    pub(crate) default_format: String,
    /// --xyz.format 是否来自命令行（第二层，高于逐命令 hint）。
    pub(crate) format_from_flag: bool,
    /// auto 交互式下半（默认 text）。
    pub(crate) format_interactive: String,
    /// auto 非交互下半（默认 jsonl）。
    pub(crate) format_piped: String,
    /// 强制交互式裁定（测试/嵌入；None=按输出目标探测，§10.7）。
    pub(crate) interactive_override: Option<bool>,
    /// 输出是否由嵌入方注入（非默认 stdout）：注入 writer 一律非交互。
    pub(crate) out_injected: bool,
}

/// 嵌入场景的前端级配置（零值保持 stdout/stderr）。
#[derive(Default)]
pub struct Options {
    pub out: Option<Box<dyn Write + Send>>,
    pub err_out: Option<Box<dyn Write + Send>>,
    /// 默认输出格式（--xyz.format / Config.Format；None/空=auto）。
    pub format: Option<String>,
    /// format 来自命令行 --xyz.format（命令行层）。
    pub format_from_flag: bool,
    /// auto 的交互式下半（空= text）。
    pub format_interactive: Option<String>,
    /// auto 的非交互下半（空= jsonl）。
    pub format_piped: Option<String>,
    /// 强制交互式裁定（测试/嵌入用；None=按输出目标探测）。
    pub interactive: Option<bool>,
}

/// 一次叶子命令执行的只读快照，交给 Execute 中间件（use_mw）。
pub struct ExecContext {
    /// 点分注册名，如 user.add。
    pub path: String,
    /// 命令元数据（Hints、InputSchema、OutputSchema）。
    pub entry: Arc<Entry>,
    /// 生效的输出格式（spec §10.7；未调 next 自行渲染时可参考）。
    pub format: Format,
    /// 机器格式（json/jsonl）——保留的兼容字段，等价 format.is_machine()。
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
            default_format: String::new(),
            format_from_flag: false,
            format_interactive: String::new(),
            format_piped: String::new(),
            interactive_override: None,
            out_injected: false,
        })
    }

    /// 带前端选项的构建；None 选项保持默认。
    pub fn new_with_options(reg: &Registry, opts: Options) -> errors::Result<App> {
        // 构建期校验代码级格式配置（对齐 Go NewWithOptions）：非法值让 CLI
        // 构建失败（run_context 打印并 exit 2），而不是静默兜底。
        for v in [
            opts.format.as_deref().unwrap_or(""),
            opts.format_interactive.as_deref().unwrap_or(""),
            opts.format_piped.as_deref().unwrap_or(""),
        ] {
            if Format::parse(v).is_none() {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!("cli: invalid format {v:?} (want auto|text|json|jsonl|markdown)"),
                ));
            }
        }
        let mut a = App::new(reg)?;
        if let Some(o) = opts.out {
            a.out = Arc::new(Mutex::new(o));
            a.out_injected = true;
        }
        if let Some(e) = opts.err_out {
            a.err_out = Arc::new(Mutex::new(e));
        }
        if let Some(f) = opts.format {
            a.default_format = f;
        }
        a.format_from_flag = opts.format_from_flag;
        if let Some(v) = opts.format_interactive {
            a.format_interactive = v;
        }
        if let Some(v) = opts.format_piped {
            a.format_piped = v;
        }
        a.interactive_override = opts.interactive;
        if a.out_injected {
            // 注入 writer 非交互（spec §10.7）：嵌入方可用 interactive 强制。
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
            self.out_injected = true;
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
            if arg == "--" {
                break; // 之后的 token 全是位置参数，-v 不再算开关
            }
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
        // 输出格式（spec §10.7）：默认取 --xyz.format（注入 default_format），
        // 裸 --format/--json 在未被目标命令同名 flag 遮蔽时覆盖之；全称
        // --xyz.format 已在派发层消费（不受遮蔽影响）。
        let target = self.resolve_target(args);
        let format_conflict = target.map(|n| node_has_flag(n, "format")).unwrap_or(false);
        let json_conflict = target.map(|n| node_has_flag(n, "json")).unwrap_or(false);
        let mut bare: Option<String> = None;
        let mut filtered: Vec<String> = Vec::with_capacity(args.len());
        let mut past_double_dash = false;
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if past_double_dash {
                filtered.push(a.clone());
                i += 1;
                continue;
            }
            match a.as_str() {
                "--" => {
                    past_double_dash = true;
                    filtered.push(a.clone());
                }
                "--json" if !json_conflict => bare = Some("json".to_string()),
                "--format" if !format_conflict => {
                    if i + 1 >= args.len() {
                        let _ = writeln!(
                            self.err_out.lock().unwrap(),
                            "{bin}: --format needs an argument (text|json|jsonl|markdown)"
                        );
                        return 2;
                    }
                    i += 1;
                    bare = Some(args[i].clone());
                }
                _ if a.starts_with("--format=") && !format_conflict => {
                    bare = Some(a["--format=".len()..].to_string());
                }
                // 被遮蔽（或未知）的裸标志原样留给命令自己的解析。
                _ => filtered.push(a.clone()),
            }
            i += 1;
        }
        if let Some(b) = &bare
            && Format::parse(b).is_none()
        {
            let _ = writeln!(
                self.err_out.lock().unwrap(),
                "{bin}: invalid output format {b:?} (want auto|text|json|jsonl|markdown)"
            );
            return 2;
        }
        let entry = target.and_then(|n| n.entry.clone());
        let format = self.resolve_format(bare.as_deref(), entry.as_deref());
        if let Err(e) = self.execute(ctx, self.root.clone(), &filtered, format, &bin) {
            // 下游早关管道（| head 等）：静默按 Go 的 SIGPIPE 惯例退出。
            if errors::is_broken_pipe(&e) {
                return 141;
            }
            self.render_error(&e, format);
            return exit_code_of(&e);
        }
        0
    }

    /// 输出目标是否交互式（spec §10.7）：强制裁定优先；嵌入注入的 writer
    /// 一律非交互；默认 stdout 走 termx 探针（与保留的样式轴共用，§10.7a）。
    pub(crate) fn interactive(&self) -> bool {
        if let Some(v) = self.interactive_override {
            return v;
        }
        if self.out_injected {
            return false;
        }
        crate::termx::interactive()
    }

    /// 五级优先级解析本次执行的具体格式（spec §10.7）：bare --format/--json
    /// （未遮蔽）> --xyz.format（命令行）> CliHints.format > Config.Format >
    /// auto（按 TTY 解析）。任一层可写 auto（或空白沿用下层）。层 1/2 的
    /// 非法值在调用方用法错误退出；层 3/4 的非法值宽松忽略（视同未设置）。
    pub(crate) fn resolve_format(&self, bare: Option<&str>, entry: Option<&Entry>) -> Format {
        let iv = self.interactive();
        let resolved = |s: &str| match Format::parse(s) {
            Some(Format::Auto) => {
                Format::Auto.resolve(iv, &self.format_interactive, &self.format_piped)
            }
            Some(f) => f,
            // 非法代码配置值：按 Go 渲染 default 语义走 text（纵深兜底；
            // 命令行两层非法在调用方与 builtins 已分别拦下）。
            None => Format::Text,
        };
        if let Some(b) = bare {
            return resolved(b);
        }
        if self.format_from_flag {
            return resolved(&self.default_format);
        }
        // 层 3/4：非空即采用（即使解析失败也停止下落，与 Go resolveFormat
        // 的"读到值即停"一致，失败值渲染时落 text）。
        if let Some(e) = entry
            && !e.cli.format.is_empty()
        {
            return resolved(&e.cli.format);
        }
        if !self.default_format.is_empty() {
            return resolved(&self.default_format);
        }
        Format::Auto.resolve(iv, &self.format_interactive, &self.format_piped)
    }

    /// 目标节点解析（§10.7 冲突让位判定用）：沿非 flag 段下沉（含默认子
    /// 命令转发），返回最深可达节点。
    fn resolve_target(&self, args: &[String]) -> Option<&CmdNode> {
        let mut node: &CmdNode = &self.root;
        let mut rest = args;
        loop {
            let Some(first) = rest.first() else {
                return Some(node);
            };
            if first == "--" || first.starts_with('-') {
                rest = &rest[1..];
                continue;
            }
            if let Some(child) = node
                .children
                .iter()
                .find(|c| c.segment == *first || (c.leaf && c.aliases.iter().any(|a| a == first)))
            {
                node = child;
                rest = &rest[1..];
                continue;
            }
            let default_seg = node.default_segment.clone();
            if let Some(seg) = default_seg
                && let Some(child) = node.children.iter().find(|c| c.segment == seg)
            {
                node = child;
                continue; // 默认子命令：不消费该段
            }
            return Some(node);
        }
    }

    /// 命令错误 → stderr（spec §10.7/§8.6）：机器格式写共享错误体
    /// （json pretty / jsonl 紧凑），其余格式写人类可读一行。
    fn render_error(&self, e: &errors::Error, format: Format) {
        let mut w = self.err_out.lock().unwrap();
        match format {
            Format::Json | Format::JsonL => {
                let body = errors::error_body(e);
                let s = if format == Format::Json {
                    serde_json::to_string_pretty(&body)
                } else {
                    serde_json::to_string(&body)
                }
                .unwrap_or_else(|_| "{\"error\":\"\"}".to_string());
                let _ = writeln!(*w, "{s}");
            }
            _ => {
                let _ = writeln!(*w, "{e}");
            }
        }
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
        format: Format,
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
        // 默认子命令：首段不是已注册命令段、也不是 flag（-h/-v 等）时，
        // 整串参数不消费地转发给默认子命令（udf img ⇔ udf extract img）。
        if !node.leaf
            && !rest.is_empty()
            && !rest[0].starts_with('-')
            && let Some(seg) = node.default_segment.clone()
            && let Some(child) = node.children.iter().find(|c| c.segment == seg)
        {
            node = child.clone();
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
                crate::lang::tf(
                    "cli.err_positional_count",
                    &[
                        &node.path.replace('.', " "),
                        &node.min_pos.to_string(),
                        &node.max_pos.to_string(),
                        &pos.len().to_string(),
                    ],
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
            format,
            json: format.is_machine(),
            out: Arc::clone(&self.out),
        };

        // 中间件洋葱链：自内向外构建（最晚注册的最外层）。
        let terminal = |ctx: &Ctx, ec: &ExecContext, args: &mut Map<String, Value>| {
            let out = (ec.entry.invoke)(ctx, args)?;
            // 长驻命令：ctx 取消即优雅关停，不渲染返回值。
            if ec.entry.cli.daemon {
                return Ok(());
            }
            let mut w = ec.out.lock().unwrap();
            match ec.format {
                // resolve 后不应到达 Auto（兜底走人类渲染）。
                Format::Auto | Format::Text => super::render::render(&mut **w, &out)?,
                Format::Json => {
                    let s = serde_json::to_string_pretty(&out).map_err(|e| {
                        errors::Error::new(
                            errors::Kind::Internal,
                            format!("result serialization: {e}"),
                        )
                    })?;
                    writeln!(*w, "{s}").map_err(io_err)?;
                }
                Format::JsonL => super::format::render_jsonl(&mut **w, &out)?,
                Format::Markdown => super::format::render_markdown(&mut **w, &out)?,
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

/// 目标命令是否定义了同名长 flag（spec §10.7 裸标志让位判定）。
fn node_has_flag(node: &CmdNode, long: &str) -> bool {
    node.leaf && node.defs.iter().any(|d| d.long == long)
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
