// 命令树：点分注册名映射为多层子命令段；别名挂在父层查找路径上
// （不出现在父帮助的命令列表里）。叶子节点携带 flag 定义、env 注入点
// 与位置参数约束。

use std::sync::Arc;

use crate::cli::parse::FlagDef;
use crate::errors;
use crate::registry::Registry;
use crate::spec::entry::Entry;
use crate::spec::field::{FieldKind, FieldMeta};

#[derive(Clone)]
pub struct CmdNode {
    /// 完整点分路径，如 user.add。
    pub path: String,
    /// 本段命令名（帮助列表与下沉匹配用）。
    pub segment: String,
    pub usage: String,
    pub short: String,
    pub long: String,
    pub aliases: Vec<String>,
    pub hidden: bool,
    pub leaf: bool,
    pub entry: Option<Arc<Entry>>,
    pub defs: Vec<FlagDef>,
    /// #[xyz(skip)] 且配置了 env 的注入字段。
    pub env_only: Vec<FieldMeta>,
    pub pos_f: Vec<FieldMeta>,
    pub min_pos: usize,
    pub max_pos: usize,
    /// 有序子节点（按段名排序；别名不占列表位）。
    pub children: Vec<CmdNode>,
    /// 默认子命令的末段名（CliHints.default）；执行期未匹配命令段时整串
    /// 参数转发给它。
    pub default_segment: Option<String>,
}

impl CmdNode {
    fn new(segment: &str) -> Self {
        CmdNode {
            path: String::new(),
            segment: segment.to_string(),
            usage: String::new(),
            short: String::new(),
            long: String::new(),
            aliases: Vec::new(),
            hidden: false,
            leaf: false,
            entry: None,
            defs: Vec::new(),
            env_only: Vec::new(),
            pos_f: Vec::new(),
            min_pos: 0,
            max_pos: 0,
            children: Vec::new(),
            default_segment: None,
        }
    }
}

/// 由注册表构建命令树。冲突（路径重叠、别名撞名、短名重复、required
/// 位置参数排在 optional 之后）全部是构建期错误。
pub fn build_tree(reg: &Registry) -> errors::Result<CmdNode> {
    let mut root = CmdNode::new("");
    for e in reg.all() {
        add_entry(&mut root, &e)?;
    }
    for child in &mut root.children {
        sort_node(child);
    }
    root.children.sort_by(|a, b| a.segment.cmp(&b.segment));
    Ok(root)
}

fn sort_node(node: &mut CmdNode) {
    for child in &mut node.children {
        sort_node(child);
    }
    node.children.sort_by(|a, b| a.segment.cmp(&b.segment));
}

fn add_entry(root: &mut CmdNode, e: &Arc<Entry>) -> errors::Result<()> {
    let parts: Vec<&str> = e.name.split('.').collect();
    add_parts(root, &parts, 0, e)
}

fn add_parts(node: &mut CmdNode, parts: &[&str], idx: usize, e: &Arc<Entry>) -> errors::Result<()> {
    if e.cli.skip {
        return Ok(()); // 通道层面整体移除：不建子命令、别名、completion
    }
    let part = parts[idx];
    let is_leaf_segment = idx == parts.len() - 1;
    if !is_leaf_segment {
        if !node.children.iter().any(|c| c.segment == part) {
            node.children.push(CmdNode::new(part));
        }
        let idx_of = node
            .children
            .iter()
            .position(|c| c.segment == part)
            .unwrap();
        return add_parts(&mut node.children[idx_of], parts, idx + 1, e);
    }

    // 叶子落点：已存在同名节点（且已是叶子或已带子树）即冲突。
    match node.children.iter().position(|c| c.segment == part) {
        Some(i) => {
            if node.children[i].leaf || !node.children[i].children.is_empty() {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!(
                        "cli: command {:?} conflicts with an existing command path",
                        e.name
                    ),
                ));
            }
            // 别名挂在父节点查找表上（等价于子命令名，不进帮助列表）。
            for alias in &e.cli.aliases {
                if node
                    .children
                    .iter()
                    .enumerate()
                    .any(|(j, c)| j != i && c.segment == *alias)
                {
                    return Err(errors::Error::new(
                        errors::Kind::Internal,
                        format!(
                            "cli: command {:?}: alias {:?} collides with an existing command path",
                            e.name, alias
                        ),
                    ));
                }
            }
            fill_leaf(&mut node.children[i], e)?;
            set_default(node, part, e)?;
            Ok(())
        }
        None => {
            for alias in &e.cli.aliases {
                if node.children.iter().any(|c| c.segment == *alias) {
                    return Err(errors::Error::new(
                        errors::Kind::Internal,
                        format!(
                            "cli: command {:?}: alias {:?} collides with an existing command path",
                            e.name, alias
                        ),
                    ));
                }
            }
            let mut leaf = CmdNode::new(part);
            fill_leaf(&mut leaf, e)?;
            node.children.push(leaf);
            set_default(node, part, e)?;
            Ok(())
        }
    }
}

/// Default：登记为父节点的默认子命令，一个父节点最多一个（注册期报错）。
fn set_default(node: &mut CmdNode, part: &str, e: &Arc<Entry>) -> errors::Result<()> {
    if !e.cli.default {
        return Ok(());
    }
    if let Some(prev) = &node.default_segment {
        if prev != part {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!(
                    "cli: command {:?}: default conflicts with existing default {:?}",
                    e.name, prev
                ),
            ));
        }
        return Ok(());
    }
    node.default_segment = Some(part.to_string());
    Ok(())
}

fn fill_leaf(node: &mut CmdNode, e: &Arc<Entry>) -> errors::Result<()> {
    node.path = e.name.clone();
    node.usage = e.cli.usage.clone();
    node.short = e.summary.clone();
    node.long = e.description.clone();
    node.aliases = e.cli.aliases.clone();
    node.hidden = e.cli.hidden;
    node.leaf = true;
    node.entry = Some(Arc::clone(e));

    let mut seen_sh: Vec<(char, String)> = Vec::new();
    for f in &e.root.children {
        if f.cli.skip {
            continue;
        }
        if f.skip {
            // json:"-" 字段不生成 flag；配置了 env 时注册纯注入点。
            if f.cli.env_var.is_some() {
                node.env_only.push(f.clone());
            }
            continue;
        }
        if f.cli.positional {
            node.pos_f.push(f.clone());
            continue;
        }
        let (kind, ok) = flag_kind_for(f, &e.name)?;
        if !ok {
            continue;
        }
        if let Some(sh) = f.cli.shorthand {
            if let Some((_, prev)) = seen_sh.iter().find(|(c, _)| *c == sh) {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!(
                        "cli: command {:?}: shorthand {sh:?} of field {:?} already used by {prev:?}",
                        e.name, f.json_name
                    ),
                ));
            }
            seen_sh.push((sh, f.json_name.clone()));
        }
        node.defs.push(FlagDef {
            long: f.json_name.clone(),
            short: f.cli.shorthand,
            kind,
            field: f.clone(),
        });
    }

    // 位置参数：required 必须是前缀，否则语义有歧义。
    let mut min_pos = 0usize;
    let mut all_required = true;
    for (i, f) in node.pos_f.iter().enumerate() {
        if f.required && !all_required {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!(
                    "cli: command {:?}: required positional {:?} must not follow optional ones",
                    e.name, f.json_name
                ),
            ));
        }
        if f.required {
            min_pos = i + 1;
        } else {
            all_required = false;
        }
    }
    node.min_pos = min_pos;
    node.max_pos = node.pos_f.len();
    Ok(())
}

/// 判定字段的 flag 类型；不支持的种类在构建时报错（Go flagKindFor 对应物）。
fn flag_kind_for(
    f: &FieldMeta,
    cmd_name: &str,
) -> errors::Result<(crate::cli::parse::FlagKind, bool)> {
    use crate::cli::parse::FlagKind::{Bool, Slice, Str};
    match f.kind {
        FieldKind::Bool => Ok((Bool, true)),
        FieldKind::Slice => {
            // []byte 除外：字节字段按字符串进（属于 scalar 路径）。
            if matches!(
                f.elem.as_deref().map(|e| e.kind),
                Some(FieldKind::Struct | FieldKind::Ptr | FieldKind::Slice)
            ) {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!(
                        "cli: command {cmd_name:?}: field {:?}: slice of {:?} is not supported by the CLI frontend yet",
                        f.json_name,
                        f.elem.as_deref().map(|e| e.kind)
                    ),
                ));
            }
            Ok((Slice, true))
        }
        FieldKind::Struct => Err(errors::Error::new(
            errors::Kind::Internal,
            format!(
                "cli: command {cmd_name:?}: field {:?}: nested struct is not supported by the CLI frontend yet",
                f.json_name
            ),
        )),
        FieldKind::Union => {
            // spec §4.7 尾部政策：CLI 无法原生表达的联合字段跳过
            // （该命令其余字段照常、整个 CLI 不受影响）。完整 flag 形态
            // 是后续迭代。
            Ok((Str, false))
        }
        FieldKind::Ptr => {
            if matches!(f.elem.as_deref().map(|e| e.kind), Some(FieldKind::Struct)) {
                return Err(errors::Error::new(
                    errors::Kind::Internal,
                    format!(
                        "cli: command {cmd_name:?}: field {:?}: pointer to struct is not supported by the CLI frontend yet",
                        f.json_name
                    ),
                ));
            }
            Ok((Str, true))
        }
        _ => Ok((Str, true)),
    }
}
