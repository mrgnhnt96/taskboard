//! Markdown to Jira's document format (ADF), for ticket descriptions: headings, paragraphs, bullet
//! and numbered lists (nested by indent), checkboxes, code blocks, quotes, rules, and inline bold,
//! italics, code and links. Anything else stays as its text.

use serde_json::{json, Value};

pub fn from_markdown(text: &str) -> Value {
    let lines: Vec<&str> = text.lines().collect();
    json!({"type": "doc", "version": 1, "content": blocks(&lines)})
}

fn blocks(lines: &[&str]) -> Vec<Value> {
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.is_empty() {
            i += 1;
        } else if let Some(lang) = fence(line) {
            let mut code = vec![];
            i += 1;
            while i < lines.len() && fence(lines[i]).is_none() {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            let mut b = json!({"type": "codeBlock", "content": []});
            if !lang.is_empty() {
                b["attrs"] = json!({"language": lang});
            }
            let code = code.join("\n");
            if !code.is_empty() {
                b["content"] = json!([{"type": "text", "text": code}]);
            }
            out.push(b);
        } else if let Some((level, rest)) = heading(t) {
            out.push(json!({"type": "heading", "attrs": {"level": level}, "content": inline(rest)}));
            i += 1;
        } else if rule(t) {
            out.push(json!({"type": "rule"}));
            i += 1;
        } else if t.starts_with('>') {
            let mut quoted = vec![];
            while i < lines.len() && lines[i].trim().starts_with('>') {
                let q = lines[i].trim().trim_start_matches('>');
                quoted.push(q.strip_prefix(' ').unwrap_or(q));
                i += 1;
            }
            let inner = blocks(&quoted);
            if !inner.is_empty() {
                out.push(json!({"type": "blockquote", "content": inner}));
            }
        } else if marker(line).is_some() {
            let mut items = vec![];
            while i < lines.len() {
                if let Some(m) = marker(lines[i]) {
                    items.push(m);
                    i += 1;
                } else if lines[i].trim().is_empty() {
                    // A blank line ends the list unless the list goes on after it.
                    let next = lines[i..].iter().position(|l| !l.trim().is_empty()).map(|n| lines[i + n]);
                    match next {
                        Some(l) if marker(l).is_some() || (indent(l) > 0 && !items.is_empty()) => i += 1,
                        _ => break,
                    }
                } else if indent(lines[i]) > 0 && !items.is_empty() {
                    let last = items.last_mut().unwrap();
                    last.text.push('\n');
                    last.text.push_str(lines[i].trim());
                    i += 1;
                } else {
                    break;
                }
            }
            let mut at = 0;
            while at < items.len() {
                let (list, next) = list(&items, at, items[at].indent);
                out.push(list);
                at = next;
            }
        } else {
            let mut para = vec![];
            while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(lines[i]) {
                para.push(lines[i].trim());
                i += 1;
            }
            out.push(paragraph(&para.join("\n")));
        }
    }
    out
}

fn starts_block(line: &str) -> bool {
    let t = line.trim();
    fence(line).is_some() || heading(t).is_some() || rule(t) || t.starts_with('>') || marker(line).is_some()
}

fn fence(line: &str) -> Option<String> {
    let t = line.trim_start();
    (indent(line) < 4 && t.starts_with("```")).then(|| t.trim_start_matches('`').trim().to_string())
}

fn heading(t: &str) -> Option<(usize, &str)> {
    let level = t.chars().take_while(|c| *c == '#').count();
    let rest = &t[level..];
    ((1..=6).contains(&level) && rest.starts_with(' ')).then(|| (level, rest.trim().trim_end_matches('#').trim_end()))
}

fn rule(t: &str) -> bool {
    let c = t.chars().next().unwrap_or(' ');
    matches!(c, '-' | '*' | '_') && t.chars().filter(|x| !x.is_whitespace()).count() >= 3 && t.chars().all(|x| x == c || x == ' ')
}

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

struct Item {
    indent: usize,
    /// The number a numbered item starts its list at; None for a bullet.
    number: Option<u64>,
    text: String,
}

fn marker(line: &str) -> Option<Item> {
    let t = line.trim_start();
    if let Some(r) = ["- ", "* ", "+ "].iter().find_map(|m| t.strip_prefix(m)) {
        return Some(Item { indent: indent(line), number: None, text: checkbox(r) });
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    let rest = &t[digits..];
    if digits == 0 || digits > 9 || !(rest.starts_with(". ") || rest.starts_with(") ")) {
        return None;
    }
    Some(Item { indent: indent(line), number: t[..digits].parse().ok(), text: checkbox(&rest[2..]) })
}

/// `[ ] x` and `[x] x` as boxes: Jira's own task lists need ids the board doesn't have.
fn checkbox(t: &str) -> String {
    if let Some(r) = t.strip_prefix("[ ] ") {
        format!("☐ {r}")
    } else if let Some(r) = t.strip_prefix("[x] ").or_else(|| t.strip_prefix("[X] ")) {
        format!("☑ {r}")
    } else {
        t.trim().to_string()
    }
}

/// The list starting at `items[at]`, its items those at `base` indent of the same kind, each holding
/// the deeper items after it; and where the next list starts.
fn list(items: &[Item], at: usize, base: usize) -> (Value, usize) {
    let ordered = items[at].number.is_some();
    let mut out = vec![];
    let mut i = at;
    while i < items.len() && items[i].indent >= base {
        if items[i].indent == base && items[i].number.is_some() != ordered {
            break;
        }
        let mut content = vec![paragraph(&items[i].text)];
        i += 1;
        while i < items.len() && items[i].indent > base {
            let (inner, next) = list(items, i, items[i].indent);
            content.push(inner);
            i = next;
        }
        out.push(json!({"type": "listItem", "content": content}));
    }
    let mut l = json!({"type": if ordered { "orderedList" } else { "bulletList" }, "content": out});
    if let Some(n) = items[at].number.filter(|n| *n != 1) {
        l["attrs"] = json!({"order": n});
    }
    (l, i)
}

/// A paragraph whose lines are kept as line breaks.
fn paragraph(text: &str) -> Value {
    let mut content = vec![];
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            content.push(json!({"type": "hardBreak"}));
        }
        content.extend(inline(line));
    }
    json!({"type": "paragraph", "content": content})
}

fn text_node(text: &str, marks: &[Value]) -> Option<Value> {
    if text.is_empty() {
        return None;
    }
    let mut n = json!({"type": "text", "text": text});
    if !marks.is_empty() {
        n["marks"] = json!(marks);
    }
    Some(n)
}

/// Adds a mark to each node, but never to code, which Jira only lets carry a link.
fn marked(nodes: Vec<Value>, mark: Value) -> Vec<Value> {
    nodes
        .into_iter()
        .map(|mut n| {
            let marks = n["marks"].as_array().cloned().unwrap_or_default();
            let code = marks.iter().any(|m| m["type"] == "code");
            if !code || mark["type"] == "link" {
                n["marks"] = json!(marks.into_iter().chain([mark.clone()]).collect::<Vec<_>>());
            }
            n
        })
        .collect()
}

fn inline(s: &str) -> Vec<Value> {
    let c: Vec<char> = s.chars().collect();
    let find = |from: usize, pat: &str| -> Option<usize> {
        let p: Vec<char> = pat.chars().collect();
        (from..c.len().saturating_sub(p.len() - 1)).find(|&k| c[k..k + p.len()] == p[..])
    };
    let sub = |a: usize, b: usize| c[a..b].iter().collect::<String>();
    let (mut out, mut buf, mut i) = (vec![], String::new(), 0);
    let flush = |buf: &mut String, out: &mut Vec<Value>| {
        out.extend(text_node(buf, &[]));
        buf.clear();
    };
    while i < c.len() {
        if c[i] == '`' {
            if let Some(j) = find(i + 1, "`").filter(|j| *j > i + 1) {
                flush(&mut buf, &mut out);
                out.extend(text_node(&sub(i + 1, j), &[json!({"type": "code"})]));
                i = j + 1;
                continue;
            }
        } else if c[i] == '[' {
            if let Some(mid) = find(i + 1, "](") {
                if let Some(end) = find(mid + 2, ")") {
                    let url = sub(mid + 2, end);
                    if !url.trim().is_empty() && !url.contains(' ') {
                        flush(&mut buf, &mut out);
                        let label = sub(i + 1, mid);
                        let label = if label.is_empty() { url.clone() } else { label };
                        out.extend(marked(inline(&label), json!({"type": "link", "attrs": {"href": url}})));
                        i = end + 1;
                        continue;
                    }
                }
            }
        } else if (c[i] == '*' || c[i] == '_') && c.get(i + 1) == Some(&c[i]) {
            let pat = sub(i, i + 2);
            if let Some(j) = find(i + 2, &pat).filter(|j| *j > i + 2) {
                flush(&mut buf, &mut out);
                out.extend(marked(inline(&sub(i + 2, j)), json!({"type": "strong"})));
                i = j + 2;
                continue;
            }
        } else if c[i] == '*' && c.get(i + 1).is_some_and(|n| !n.is_whitespace()) {
            if let Some(j) = find(i + 1, "*").filter(|j| *j > i + 1 && !c[j - 1].is_whitespace()) {
                flush(&mut buf, &mut out);
                out.extend(marked(inline(&sub(i + 1, j)), json!({"type": "em"})));
                i = j + 1;
                continue;
            }
        } else if (i == 0 || c[i - 1].is_whitespace() || c[i - 1] == '(') && (sub(i, c.len()).starts_with("https://") || sub(i, c.len()).starts_with("http://")) {
            let mut j = i;
            while j < c.len() && !c[j].is_whitespace() {
                j += 1;
            }
            while j > i && matches!(c[j - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')') {
                j -= 1;
            }
            flush(&mut buf, &mut out);
            let url = sub(i, j);
            out.extend(text_node(&url, &[json!({"type": "link", "attrs": {"href": url}})]));
            i = j;
            continue;
        }
        buf.push(c[i]);
        i += 1;
    }
    flush(&mut buf, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(md: &str) -> Vec<Value> {
        from_markdown(md)["content"].as_array().unwrap().clone()
    }

    #[test]
    fn headings_paragraphs_and_line_breaks() {
        let d = doc("## Background\nfirst line\nsecond line\n\nNext para");
        assert_eq!(d[0], json!({"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Background"}]}));
        assert_eq!(
            d[1]["content"],
            json!([{"type": "text", "text": "first line"}, {"type": "hardBreak"}, {"type": "text", "text": "second line"}])
        );
        assert_eq!(d[2]["content"][0]["text"], "Next para");
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn nested_bullets_numbers_and_checkboxes() {
        let d = doc("- one\n  - inner\n- [ ] todo\n- [x] done\n\n3. three\n4. four");
        assert_eq!(d[0]["type"], "bulletList");
        let items = d[0]["content"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["content"][1]["type"], "bulletList");
        assert_eq!(items[0]["content"][1]["content"][0]["content"][0]["content"][0]["text"], "inner");
        assert_eq!(items[1]["content"][0]["content"][0]["text"], "☐ todo");
        assert_eq!(items[2]["content"][0]["content"][0]["text"], "☑ done");
        assert_eq!(d[1]["type"], "orderedList");
        assert_eq!(d[1]["attrs"]["order"], 3);
        assert_eq!(d[1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn code_quotes_and_rules() {
        let d = doc("```rust\nfn x() {}\n```\n> quoted\n---\nafter");
        assert_eq!(d[0], json!({"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "fn x() {}"}]}));
        assert_eq!(d[1]["type"], "blockquote");
        assert_eq!(d[1]["content"][0]["content"][0]["text"], "quoted");
        assert_eq!(d[2]["type"], "rule");
        assert_eq!(d[3]["content"][0]["text"], "after");
    }

    #[test]
    fn inline_marks() {
        let n = inline("a **bold `c`** *em* [site](https://x.dev) see https://y.dev/p.");
        let marks = |i: usize| n[i]["marks"].as_array().unwrap().iter().map(|m| m["type"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(n[1]["text"], "bold ");
        assert_eq!(marks(1), ["strong"]);
        assert_eq!(n[2]["text"], "c");
        assert_eq!(marks(2), ["code"]);
        assert_eq!(n[4]["text"], "em");
        assert_eq!(marks(4), ["em"]);
        assert_eq!(n[6]["marks"][0]["attrs"]["href"], "https://x.dev");
        assert_eq!(n[8]["text"], "https://y.dev/p");
        assert_eq!(n[9]["text"], ".");
    }

    #[test]
    fn plain_text_and_snake_case_stay_as_they_are() {
        let n = inline("my_var_name * 2 = x");
        assert_eq!(n, vec![json!({"type": "text", "text": "my_var_name * 2 = x"})]);
    }
}
