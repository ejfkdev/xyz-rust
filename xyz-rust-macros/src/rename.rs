// serde rename_all 风格的最小实现（camelCase / snake_case /
// PascalCase / kebab-case / SCREAMING_SNAKE_CASE / lowercase / UPPERCASE）。
// 字段名默认 snake_case，转换按词根拆分。

pub fn apply(style: &str, input: &str) -> String {
    let words: Vec<String> = split_words(input);
    match style {
        "camelCase" => camel(&words),
        "snake_case" => words.join("_").to_lowercase(),
        "PascalCase" => words.iter().map(|w| capitalize(w)).collect(),
        "kebab-case" => words.join("-").to_lowercase(),
        "SCREAMING_SNAKE_CASE" => words.join("_").to_uppercase(),
        "lowercase" => words.concat().to_lowercase(),
        "UPPERCASE" => words.concat().to_uppercase(),
        _ => input.to_string(), // 未识别风格：原样（Go 无名对应物，serde 行为一致）
    }
}

/// 按 snake_case / camelCase 边界拆词。
fn split_words(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = input.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '_' || *c == '-' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if c.is_uppercase()
            && !cur.is_empty()
            && !cur.chars().last().unwrap().is_uppercase()
            && !cur.chars().last().unwrap().is_numeric()
        {
            out.push(std::mem::take(&mut cur));
            cur.push(*c);
            continue;
        }
        // 连续大写后跟小写：HTMLParser -> HTML / Parser
        if c.is_uppercase()
            && i > 0
            && chars[i - 1].is_uppercase()
            && i + 1 < chars.len()
            && chars[i + 1].is_lowercase()
            && !cur.is_empty()
            && cur.chars().count() > 1
        {
            let last = cur.pop().unwrap();
            out.push(std::mem::take(&mut cur));
            cur.push(last);
        }
        cur.push(*c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() && !input.is_empty() {
        out.push(input.to_string());
    }
    out.into_iter().map(|w| w.to_lowercase()).collect()
}

fn camel(words: &[String]) -> String {
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i == 0 {
            out.push_str(&w.to_lowercase());
        } else {
            out.push_str(&capitalize(w));
        }
    }
    out
}

fn capitalize(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
