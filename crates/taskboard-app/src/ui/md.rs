//! The web board's `mdLite`, ported step for step: paragraphs (lines joined by line breaks),
//! headings, bullet and numbered lists (with indented continuation lines), fenced code, and the
//! inline pass (`code`, **bold**, *italic*, [links](https://…) and bare https links), cut at
//! `max` characters.
//!
//! The inline pass is the web's exact regex pipeline run on the HTML-escaped line, producing the
//! same markup (`<code>`, `<b>`, `<i>`, `<a href>`); [`to_html`] returns exactly what `mdLite`
//! returned (the parity tests compare it), and [`render`] draws the same markup with GPUI.
use crate::theme::Theme;
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Debug, PartialEq)]
enum Block {
    /// Lines, each already through the inline pass (markup).
    Para(Vec<String>),
    Heading(String),
    List { ordered: bool, items: Vec<String> },
    /// Raw code (escaped only when written out as HTML).
    Code(String),
}

// ------------------------------------------------------------------ the JS bits

/// JS `\s`.
fn js_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{a0}' | '\u{1680}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
        || ('\u{2000}'..='\u{200a}').contains(&c)
}

/// JS `\w`.
fn js_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// JS `String.prototype.trim`.
fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '&' => o.push_str("&amp;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

/// `text.replace(/\r\n?/g, '\n').trim()`, then the cut to `max` UTF-16 units at a late newline.
fn prepare(text: &str, max: usize) -> String {
    let src = js_trim(&text.replace("\r\n", "\n").replace('\r', "\n")).to_string();
    let units: Vec<u16> = src.encode_utf16().collect();
    if units.len() <= max {
        return src;
    }
    // src.lastIndexOf('\n', max): the last newline at index <= max.
    let cut = units[..=max.min(units.len() - 1)].iter().rposition(|&u| u == b'\n' as u16);
    let at = match cut {
        Some(c) if c as f64 > max as f64 * 0.6 => c,
        _ => max,
    };
    let head = String::from_utf16_lossy(&units[..at]);
    format!("{} …", head.trim_end_matches(js_space))
}

/// One global regex replace done the JS way: scan left to right; at each start try `m`, which
/// returns (match end, replacement); on a match continue after it, else move one char on.
fn replace_all(s: &str, m: impl Fn(&[char], usize) -> Option<(usize, String)>) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < cs.len() {
        match m(&cs, i) {
            Some((end, rep)) if end > i => {
                out.push_str(&rep);
                i = end;
            }
            _ => {
                out.push(cs[i]);
                i += 1;
            }
        }
    }
    out
}

fn collect(cs: &[char], a: usize, b: usize) -> String {
    cs[a..b].iter().collect()
}

/// The web's `inline`: esc, then code, bold, italic, links, bare links — each a global replace
/// over the previous result (so later steps also see earlier steps' markup, as in the web).
pub fn inline(t: &str) -> String {
    let s = esc(t);
    // /`([^`\n]+)`/g
    let s = replace_all(&s, |cs, i| {
        if cs[i] != '`' {
            return None;
        }
        let mut j = i + 1;
        while j < cs.len() && cs[j] != '`' && cs[j] != '\n' {
            j += 1;
        }
        (j < cs.len() && cs[j] == '`' && j > i + 1).then(|| (j + 1, format!("<code>{}</code>", collect(cs, i + 1, j))))
    });
    // /\*\*([^*\n]+)\*\*/g
    let s = replace_all(&s, |cs, i| {
        if cs.get(i) != Some(&'*') || cs.get(i + 1) != Some(&'*') {
            return None;
        }
        let mut j = i + 2;
        while j < cs.len() && cs[j] != '*' && cs[j] != '\n' {
            j += 1;
        }
        (j > i + 2 && cs.get(j) == Some(&'*') && cs.get(j + 1) == Some(&'*')).then(|| (j + 2, format!("<b>{}</b>", collect(cs, i + 2, j))))
    });
    // /(^|[^\w*])\*([^*\n]+)\*(?!\w)/g  — group 1 is the char before (or the string start).
    let s = replace_all(&s, |cs, i| {
        let (lead, star) = if i == 0 && cs.first() == Some(&'*') {
            (String::new(), 0)
        } else if !js_word(cs[i]) && cs[i] != '*' && cs.get(i + 1) == Some(&'*') {
            (cs[i].to_string(), i + 1)
        } else {
            return None;
        };
        let mut j = star + 1;
        while j < cs.len() && cs[j] != '*' && cs[j] != '\n' {
            j += 1;
        }
        let ok = j > star + 1 && cs.get(j) == Some(&'*') && !cs.get(j + 1).is_some_and(|c| js_word(*c));
        ok.then(|| (j + 1, format!("{lead}<i>{}</i>", collect(cs, star + 1, j))))
    });
    // /\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g
    let s = replace_all(&s, |cs, i| {
        if cs[i] != '[' {
            return None;
        }
        let mut j = i + 1;
        while j < cs.len() && cs[j] != ']' && cs[j] != '\n' {
            j += 1;
        }
        if j == i + 1 || cs.get(j) != Some(&']') || cs.get(j + 1) != Some(&'(') {
            return None;
        }
        let u0 = j + 2;
        let scheme = scheme_len(cs, u0)?;
        let mut k = u0 + scheme;
        while k < cs.len() && !js_space(cs[k]) && cs[k] != ')' {
            k += 1;
        }
        if k == u0 + scheme || cs.get(k) != Some(&')') {
            return None;
        }
        let url = collect(cs, u0, k);
        Some((k + 1, format!("<a href=\"{url}\" target=\"_blank\" rel=\"noopener\">{}</a>", collect(cs, i + 1, j))))
    });
    // /(^|[\s(])(https?:\/\/[^\s<)]+[^\s<).,;:!?])/g
    replace_all(&s, |cs, i| {
        let (lead, u0) = if i == 0 && scheme_len(cs, 0).is_some() {
            (String::new(), 0)
        } else if js_space(cs[i]) || cs[i] == '(' {
            (cs[i].to_string(), i + 1)
        } else {
            return None;
        };
        let scheme = scheme_len(cs, u0)?;
        let r0 = u0 + scheme;
        let mut k = r0;
        while k < cs.len() && !js_space(cs[k]) && cs[k] != '<' && cs[k] != ')' {
            k += 1;
        }
        // Longest run, backing off until the last char isn't punctuation, keeping >= 2 chars.
        let mut end = k;
        while end >= r0 + 2 && matches!(cs[end - 1], '.' | ',' | ';' | ':' | '!' | '?') {
            end -= 1;
        }
        if end < r0 + 2 {
            return None;
        }
        let url = collect(cs, u0, end);
        Some((end, format!("{lead}<a href=\"{url}\" target=\"_blank\" rel=\"noopener\">{url}</a>")))
    })
}

/// `https://` or `http://` at `i`: its length.
fn scheme_len(cs: &[char], i: usize) -> Option<usize> {
    for p in ["https://", "http://"] {
        if cs.len() >= i + p.len() && cs[i..i + p.len()].iter().copied().eq(p.chars()) {
            return Some(p.len());
        }
    }
    None
}

/// `/^#{1,6}\s+(.*)$/` → the heading text.
fn heading(line: &str) -> Option<String> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    let body = rest.trim_start_matches(js_space);
    (body.len() < rest.len()).then(|| body.to_string())
}

/// `/^\s*(?:([-*+])|(\d+)[.)])\s+(.*)$/` → (ordered, text).
fn list_item(line: &str) -> Option<(bool, String)> {
    let l = line.trim_start_matches(js_space);
    let mut cs = l.chars();
    let (ordered, rest) = match cs.next()? {
        '-' | '*' | '+' => (false, cs.as_str()),
        c if c.is_ascii_digit() => {
            let digits = l.chars().take_while(|c| c.is_ascii_digit()).count();
            let r = &l[digits..];
            let r = r.strip_prefix('.').or_else(|| r.strip_prefix(')'))?;
            (true, r)
        }
        _ => return None,
    };
    let body = rest.trim_start_matches(js_space);
    (body.len() < rest.len()).then(|| (ordered, body.to_string()))
}

fn parse(text: &str, max: usize) -> Vec<Block> {
    let src = prepare(text, max);
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut list: Option<(bool, Vec<String>)> = None;
    let mut code: Option<Vec<String>> = None;
    fn flush(out: &mut Vec<Block>, para: &mut Vec<String>, list: &mut Option<(bool, Vec<String>)>) {
        if !para.is_empty() {
            out.push(Block::Para(std::mem::take(para).iter().map(|l| inline(l)).collect()));
        }
        if let Some((ordered, items)) = list.take() {
            out.push(Block::List { ordered, items: items.iter().map(|i| inline(i)).collect() });
        }
    }
    let fence = |l: &str| l.trim_start_matches(js_space).starts_with("```");
    for line in src.split('\n') {
        if let Some(c) = code.as_mut() {
            if fence(line) {
                out.push(Block::Code(code.take().unwrap_or_default().join("\n")));
            } else {
                c.push(line.to_string());
            }
            continue;
        }
        if fence(line) {
            flush(&mut out, &mut para, &mut list);
            code = Some(Vec::new());
            continue;
        }
        if js_trim(line).is_empty() {
            flush(&mut out, &mut para, &mut list);
        } else if let Some(h) = heading(line) {
            flush(&mut out, &mut para, &mut list);
            out.push(Block::Heading(inline(&h)));
        } else if let Some((ordered, item)) = list_item(line) {
            if !para.is_empty() || list.as_ref().is_some_and(|(o, _)| *o != ordered) {
                flush(&mut out, &mut para, &mut list);
            }
            list.get_or_insert((ordered, Vec::new())).1.push(item);
        } else if list.is_some() && line.starts_with(js_space) && !js_trim(line).is_empty() {
            if let Some(last) = list.as_mut().and_then(|(_, items)| items.last_mut()) {
                last.push(' ');
                last.push_str(js_trim(line));
            }
        } else {
            if list.is_some() {
                flush(&mut out, &mut para, &mut list);
            }
            para.push(line.to_string());
        }
    }
    if let Some(c) = code {
        out.push(Block::Code(c.join("\n")));
    }
    flush(&mut out, &mut para, &mut list);
    out
}

/// Exactly what the web's `mdLite(text, max)` returned (the parity tests compare it).
#[cfg(test)]
fn to_html(text: &str, max: usize) -> String {
    let mut o = String::from("<div class=\"md\">");
    for b in parse(text, max) {
        match b {
            Block::Para(lines) => o.push_str(&format!("<p>{}</p>", lines.join("<br>"))),
            Block::Heading(h) => o.push_str(&format!("<p class=\"md-h\">{h}</p>")),
            Block::List { ordered, items } => {
                let tag = if ordered { "ol" } else { "ul" };
                o.push_str(&format!("<{tag}>{}</{tag}>", items.iter().map(|i| format!("<li>{i}</li>")).collect::<String>()));
            }
            Block::Code(c) => o.push_str(&format!("<pre><code>{}</code></pre>", esc(&c))),
        }
    }
    o.push_str("</div>");
    o
}

// ------------------------------------------------------------------ drawing the markup

#[derive(Clone, Default, PartialEq, Debug)]
struct Span {
    text: String,
    code: bool,
    bold: bool,
    italic: bool,
    link: Option<String>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// Markup from [`inline`] → styled spans (tags nest: a link around code, bold inside code…).
fn spans(markup: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut cur = Span::default();
    let mut stack: Vec<(&'static str, Option<String>)> = Vec::new();
    let mut rest = markup;
    let apply = |stack: &[(&'static str, Option<String>)]| {
        let mut s = Span::default();
        for (t, u) in stack {
            match *t {
                "code" => s.code = true,
                "b" => s.bold = true,
                "i" => s.italic = true,
                "a" => s.link = u.clone(),
                _ => {}
            }
        }
        s
    };
    while !rest.is_empty() {
        let tag = ["<code>", "</code>", "<b>", "</b>", "<i>", "</i>", "</a>"].into_iter().find(|t| rest.starts_with(t)).map(str::to_string).or_else(|| {
            rest.strip_prefix("<a href=\"").and_then(|r| r.find("\">").map(|e| rest[..9 + e + 2].to_string()))
        });
        match tag {
            Some(t) => {
                if !cur.text.is_empty() {
                    cur.text = unescape(&cur.text);
                    out.push(std::mem::take(&mut cur));
                }
                if let Some(close) = t.strip_prefix("</") {
                    let name = close.trim_end_matches('>');
                    if let Some(i) = stack.iter().rposition(|(n, _)| *n == name) {
                        stack.remove(i);
                    }
                } else if t.starts_with("<a ") {
                    let url = t["<a href=\"".len()..t.find("\" target").unwrap_or(t.len() - 2)].to_string();
                    stack.push(("a", Some(unescape(&url))));
                } else {
                    let name: &'static str = match t.as_str() {
                        "<code>" => "code",
                        "<b>" => "b",
                        _ => "i",
                    };
                    stack.push((name, None));
                }
                cur = apply(&stack);
                rest = &rest[t.len()..];
            }
            None => {
                let ch = rest.chars().next().unwrap_or(' ');
                cur.text.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    if !cur.text.is_empty() {
        cur.text = unescape(&cur.text);
        out.push(cur);
    }
    out
}

fn inline_el(t: &Theme, markup: &str, id: &str) -> AnyElement {
    let parts = spans(markup);
    if parts.iter().all(|p| *p == Span { text: p.text.clone(), ..Default::default() }) {
        return div().child(parts.into_iter().map(|p| p.text).collect::<String>()).into_any_element();
    }
    let mut text = String::new();
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for p in parts {
        let start = text.len();
        text.push_str(&p.text);
        let r = start..text.len();
        let mut h = HighlightStyle::default();
        if p.code {
            h.background_color = Some(t.panel_2);
        }
        if p.bold {
            h.font_weight = Some(FontWeight::BOLD);
        }
        if p.italic {
            h.font_style = Some(FontStyle::Italic);
        }
        if let Some(u) = &p.link {
            h.color = Some(t.accent);
            h.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(t.accent), wavy: false });
            links.push((r.clone(), u.clone()));
        }
        if h != HighlightStyle::default() {
            highlights.push((r, h));
        }
    }
    let styled = StyledText::new(text).with_highlights(highlights);
    if links.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<_> = links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = links.into_iter().map(|(_, u)| u).collect();
    InteractiveText::new(SharedString::from(id.to_string()), styled)
        .on_click(ranges, move |ix, _, cx| {
            if let Some(u) = urls.get(ix) {
                cx.open_url(u);
            }
        })
        .into_any_element()
}

/// Render `text` as a column of blocks. `id` must be unique in the window (links need it).
pub fn render(t: &Theme, text: &str, max: usize, id: &str) -> Div {
    let mut col = div().flex().flex_col().gap(px(6.)).min_w_0();
    for (n, block) in parse(text, max).into_iter().enumerate() {
        let bid = format!("{id}-{n}");
        col = col.child(match block {
            Block::Para(lines) => {
                let mut p = div().flex().flex_col();
                for (k, l) in lines.iter().enumerate() {
                    p = p.child(inline_el(t, l, &format!("{bid}-{k}")));
                }
                p.into_any_element()
            }
            Block::Heading(h) => div().font_weight(FontWeight::BOLD).child(inline_el(t, &h, &bid)).into_any_element(),
            Block::List { ordered, items } => {
                let mut l = div().flex().flex_col().gap(px(2.));
                for (k, it) in items.iter().enumerate() {
                    let mark = if ordered { format!("{}.", k + 1) } else { "•".into() };
                    l = l.child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(div().flex_none().w(px(16.)).text_color(t.muted).child(mark))
                            .child(div().flex_1().min_w_0().child(inline_el(t, it, &format!("{bid}-{k}")))),
                    );
                }
                l.into_any_element()
            }
            Block::Code(c) => div()
                .p(px(10.))
                .rounded(px(8.))
                .bg(t.panel_2)
                .font_family(t.mono_font.clone())
                .text_size(px(12.))
                .overflow_hidden()
                .child(c)
                .into_any_element(),
        });
    }
    col
}

#[cfg(test)]
mod tests {
    use super::{Span, spans, to_html};
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn md_lite_matches_web() {
        crate::parity::golden("chrome").only("mdLite").check(|i| json!(to_html(i["text"].as_str().unwrap(), i["max"].as_u64().unwrap() as usize)));
    }

    #[::core::prelude::v1::test]
    fn markup_to_spans() {
        let s = spans("a <code>c</code> <a href=\"https://x.io\" target=\"_blank\" rel=\"noopener\"><code>k</code></a> &amp; <b>b<i>i</i></b>");
        assert_eq!(s[0].text, "a ");
        assert!(s[1].code && s[1].text == "c");
        assert!(s[3].code && s[3].link.as_deref() == Some("https://x.io") && s[3].text == "k");
        assert_eq!(s[4].text, " & ");
        assert!(s[5].bold && !s[5].italic);
        assert!(s[6].bold && s[6].italic && s[6].text == "i");
        assert_eq!(spans("plain").len(), 1);
        let _ = Span::default();
    }
}
