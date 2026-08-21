// shell 补全脚本生成（bash/zsh/fish）。脚本模板与 Go 版逐字一致；
// 词表 = 各命令顶层段 + 内建词（completion/help/serve/mcp）与通用 flag。

use std::fmt::Write as _;
use std::io::Write;

pub fn print_completion(
    top_words: &[String],
    out: &mut dyn Write,
    err_out: &mut dyn Write,
    bin: &str,
    shell: &str,
) -> i32 {
    let mut names: Vec<&String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for w in top_words {
        if !seen.contains(&w.as_str()) {
            seen.push(w.as_str());
            names.push(w);
        }
    }
    let mut list: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    list.extend(
        [
            "completion",
            "help",
            "serve",
            "mcp",
            "-h",
            "--help",
            "-v",
            "--version",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let words = list.join(" ");
    let mut buf = String::new();
    match shell {
        "bash" => {
            let _ = write!(
                buf,
                "# {bin} completion for bash\n_{bin}_completions() {{\n  local cur\n  cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  COMPREPLY=( $(compgen -W \"{words}\" -- \"$cur\") )\n}}\ncomplete -F _{bin}_completions {bin}\n"
            );
        }
        "zsh" => {
            let _ = write!(
                buf,
                "#compdef {bin}\n_{bin}_completions() {{\n  local -a cmds\n  cmds=({words})\n  _describe '{bin}' cmds\n}}\ncompdef _{bin}_completions {bin}\n"
            );
        }
        "fish" => {
            for n in &list[..list.len().saturating_sub(6)] {
                let _ = writeln!(buf, "complete -c {bin} -f -a {n}");
            }
            let _ = writeln!(buf, "complete -c {bin} -f -a completion");
        }
        _ => {
            let _ = writeln!(err_out, "unknown shell {shell:?} (want bash|zsh|fish)");
            return 2;
        }
    }
    let _ = write!(out, "{buf}");
    0
}
