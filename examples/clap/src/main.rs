// 迁移示例（Go examples/cobra/main.go 的「档位 A」对应物）：已有 clap
// 工程时，把 xyz-rust 的定义渲染成 clap 命令树。
//
// 这是三种共存方式中的「全替代」：保留你的 clap 架构、帮助样式与扩展，
// 只复用 xyz-rust 的定义层（Entry.root 元数据）与调用脊柱（Entry.invoke）。
//
// 	cargo run -p xyz-clap -- add bob -a 20        # 短名/位置参数/描述全部来自 xyz 定义
// 	APP_KEY=k cargo run -p xyz-clap -- add alice  # env 注入照常（#[xyz(skip)] 字段不走 flag）
//
// 与 Go 版用 spec.Define 一样，这里用显式 API：xyz_rust::spec::command::Command
// 打开定义，而不是 define 链。

use std::sync::Arc;

use clap::{Arg, ArgAction, error::ErrorKind as ClapErrorKind};
use serde::Serialize;
use xyz_rust::errs;
use xyz_rust::serde_json::Value;
use xyz_rust::spec::command::Command;
use xyz_rust::spec::{FieldKind, FieldMeta, JsonMap};
use xyz_rust::{CliHints, Ctx, Entry, XyzArgs};

// #[derive(XyzArgs)] 已为 AddArgs 生成 XyzSchema 实现，可直接当响应类型
// （Go examples/cobra 的 add 也是原样返回入参）——不需要再叠加 XyzOutput。
#[derive(Clone, Serialize, XyzArgs)]
struct AddArgs {
    #[xyz(desc = "用户名", required, validate = "min=2", cli = "positional")]
    name: String,
    #[xyz(desc = "年龄", default = "18", cli = "shorthand=a")]
    age: i32,
    #[xyz(desc = "标签")]
    tags: Vec<String>,
    #[xyz(skip, secret, desc = "密钥", cli = "env=APP_KEY")]
    #[serde(skip_serializing_if = "String::is_empty")]
    key: String,
}

fn add(_: &Ctx, in_: &AddArgs) -> errs::Result<AddArgs> {
    Ok(in_.clone())
}

/// entry_to_clap 把一条 xyz 定义映射成 clap::Command。要点：
///  1. 短名/位置参数/env/描述来自 Entry.root（与 xyz CLI 前端同源元数据）；
///  2. flag 值归约成 JsonMap 后交给 Entry.invoke——这是所有适配器的通用脊柱；
///  3. 渲染用 xyz_rust::cli::render，也可换成你自己的格式。
fn entry_to_clap(e: &Arc<Entry>) -> clap::Command {
    let leaf = e.name.rsplit('.').next().unwrap_or(&e.name).to_string();
    let mut cmd = clap::Command::new(leaf).about(e.summary.clone());
    if !e.cli.usage.is_empty() {
        cmd = cmd.override_usage(e.cli.usage.clone());
    }
    for alias in &e.cli.aliases {
        cmd = cmd.visible_alias(alias.clone());
    }

    // 位置参数与 env 注入字段（json:"-" 对应物）都不进 flag 列表。
    let mut positionals: Vec<&FieldMeta> = Vec::new();
    for f in &e.root.children {
        if f.skip {
            // skip 字段只管 env 注入，不做 flag
            continue;
        }
        if f.cli.positional {
            positionals.push(f);
            continue;
        }
        cmd = cmd.arg(flag_of(f));
    }
    for (i, f) in positionals.iter().enumerate() {
        cmd = cmd.arg(
            Arg::new(f.json_name.clone())
                .help(f.description.clone())
                .index(i + 1),
        );
    }
    cmd
}

fn flag_of(f: &FieldMeta) -> Arg {
    let a = Arg::new(f.json_name.clone())
        .help(f.description.clone())
        .long(f.json_name.clone());
    let a = a.with_shorthand(f.cli.shorthand);
    match f.kind {
        // Go 的 BoolVarP 对应物：出现即 true，未出现不注入
        FieldKind::Bool => a.action(ArgAction::SetTrue),
        // Go 的 StringSliceVarP 对应物：逗号分隔、可重复
        FieldKind::Slice => a.action(ArgAction::Append).value_delimiter(','),
        // Go 的 StringVarP 对应物：字符串 flag，值由 Invoke 强类型解码
        _ => a,
    }
}

trait WithShorthand: Sized {
    fn with_shorthand(self, sh: Option<char>) -> Self;
}

impl WithShorthand for Arg {
    fn with_shorthand(self, sh: Option<char>) -> Self {
        match sh {
            Some(sh) => self.short(sh),
            None => self,
        }
    }
}

fn run_entry(e: &Arc<Entry>, m: &clap::ArgMatches) -> Result<(), clap::Error> {
    // 1. CLI 专属默认值铺底（与 xyz CLI 前端注入语义一致）
    let mut values: JsonMap = e.cli_defaults();

    // 2. flag 值：只在用户显式给出时注入（对齐 Go 的 Changed 检查）
    for f in &e.root.children {
        if f.skip || f.cli.positional {
            continue;
        }
        let name = &f.json_name;
        match f.kind {
            FieldKind::Bool => {
                if m.get_flag(name) {
                    values.insert(name.clone(), Value::Bool(true));
                }
            }
            FieldKind::Slice => {
                if let Some(vs) = m.get_many::<String>(name) {
                    values.insert(
                        name.clone(),
                        Value::Array(vs.cloned().map(Value::String).collect()),
                    );
                }
            }
            _ => {
                if let Some(sv) = m.get_one::<String>(name) {
                    values.insert(name.clone(), Value::String(sv.clone()));
                }
            }
        }
    }

    // 3. env 注入：skip（json:"-"）字段按 Rust 字段名投递（与库 CLI 前端同键）
    for f in &e.root.children {
        if !f.skip {
            continue;
        }
        if let Some(ev) = &f.cli.env_var
            && let Ok(v) = std::env::var(ev)
            && !v.is_empty()
        {
            values.insert(f.name.clone(), Value::String(v));
        }
    }

    // 4. 位置参数按声明序注入；数量校验用 required 前缀（与 xyz 前端同一约束）
    let positionals: Vec<&FieldMeta> = e
        .root
        .children
        .iter()
        .filter(|f| !f.skip && f.cli.positional)
        .collect();
    let mut min_pos = 0usize;
    let mut all_req = true;
    for (i, f) in positionals.iter().enumerate() {
        if f.required && all_req {
            min_pos = i + 1;
        } else {
            all_req = false;
        }
    }
    let mut provided = 0usize;
    for f in &positionals {
        if let Some(v) = m.get_one::<String>(&f.json_name) {
            values.insert(f.json_name.clone(), Value::String(v.clone()));
            provided += 1;
        }
    }
    if provided < min_pos {
        return Err(clap::Error::raw(
            ClapErrorKind::MissingRequiredArgument,
            format!("需要至少 {min_pos} 个位置参数（提供了 {provided} 个）"),
        ));
    }

    // 5. 通用脊柱：归约出的 map 直接进 Invoke，输出走 xyz 渲染
    let ctx = Ctx::new();
    match (e.invoke)(&ctx, &values) {
        Ok(out) => {
            let mut stdout = std::io::stdout().lock();
            xyz_rust::cli::render(&mut stdout, &out)
                .map_err(|err| clap::Error::raw(ClapErrorKind::ValueValidation, err.to_string()))
        }
        Err(err) => Err(clap::Error::raw(
            ClapErrorKind::ValueValidation,
            err.to_string(),
        )),
    }
}

fn main() {
    let reg = xyz_rust::Registry::new();
    Command::new("user.add", add)
        .summary("添加用户")
        .cli(CliHints {
            usage: "add <name>".into(),
            aliases: vec!["ua".into()],
            ..Default::default()
        })
        .register(&reg)
        .expect("register user.add");

    let mut root = clap::Command::new("xyz-clap").about(
        "xyz 定义驱动的 clap 命令树（档位 A：保留 clap 架构，复用定义层元数据与 Invoke 脊柱）",
    );
    let entries: Vec<Arc<Entry>> = reg.all();
    for e in &entries {
        root = root.subcommand(entry_to_clap(e));
    }

    let matches = root.clone().get_matches();
    for e in &entries {
        let leaf = e.name.rsplit('.').next().unwrap_or(&e.name).to_string();
        if let Some(sub) = matches.subcommand_matches(&leaf) {
            match run_entry(e, sub) {
                Ok(()) => return,
                Err(err) => {
                    // 与 Go cobra 对齐：错误经 clap 渲染后以 1 退出
                    let _ = err.print();
                    std::process::exit(1);
                }
            }
        }
    }
    // 未命中任何子命令：-h/--help 已由 clap 处理，这里兜底打印帮助
    let _ = root.print_help();
}
