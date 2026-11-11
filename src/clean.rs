//! Turn GitHub-flavoured comment bodies into Markdown that Obsidian renders well.
//!
//! Bots (CodeScene, Copilot, Sonar, ...) mix HTML into their comments: `<details>`,
//! `<summary>`, `<picture>`, badge images, `<strong>` and so on. Obsidian does not render
//! Markdown inside HTML blocks (and not at all inside callouts), so the HTML is converted to
//! plain Markdown, badge images are dropped and headings are tamed. Code blocks are untouched.

/// HTML tags we understand. Anything else (e.g. `Vec<String>` in prose) is left alone.
const KNOWN_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "center",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "g-emoji",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "ol",
    "p",
    "picture",
    "pre",
    "s",
    "samp",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "u",
    "ul",
];

/// Clean a comment body. `in_callout`: headings become bold lines (they look huge inside
/// callouts); otherwise headings are demoted two levels so they sit below the note's own.
pub fn github_markdown(body: &str, in_callout: bool) -> String {
    let body = strip_html_comments(&body.replace("\r\n", "\n"));
    let mut out: Vec<String> = vec![];
    let mut prose: Vec<&str> = vec![];
    let mut in_code = false;
    let flush = |prose: &mut Vec<&str>, out: &mut Vec<String>| {
        if !prose.is_empty() {
            let text = remove_images(&html_to_md(&prose.join("\n")));
            out.extend(text.lines().filter_map(|l| prose_line(l, in_callout)));
            prose.clear();
        }
    };
    for line in body.lines() {
        let fence = line.trim_start().starts_with("```");
        if fence && !in_code {
            flush(&mut prose, &mut out);
            in_code = true;
            if line.trim() == "```suggestion" {
                out.push("**Suggested change:**".into());
                out.push("```".into());
                continue;
            }
            out.push(line.to_string());
        } else if in_code {
            out.push(line.to_string());
            in_code = !fence;
        } else {
            prose.push(line);
        }
    }
    flush(&mut prose, &mut out);

    // Collapse runs of blank lines.
    let mut result: Vec<String> = vec![];
    for line in out {
        if line.trim().is_empty() && result.last().is_none_or(|l: &String| l.trim().is_empty()) {
            continue;
        }
        result.push(line);
    }
    result.join("\n").trim().to_string()
}

pub fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Per-line fix-ups outside code blocks. `None` drops the line.
fn prose_line(line: &str, in_callout: bool) -> Option<String> {
    let line = line.trim_end();
    // A single leading space is meaningless in Markdown (often left by a removed badge).
    let line = match line.strip_prefix(' ') {
        Some(rest) if !rest.starts_with(' ') => rest,
        _ => line,
    };
    let t = line.trim_start();
    // Markdown "comments" like `[//]: # (cs-code-health)`
    if t.starts_with("[//]:") {
        return None;
    }
    if t.starts_with('#') {
        let level = t.chars().take_while(|c| *c == '#').count();
        let rest = &t[level..];
        if level <= 6 && (rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            let text = rest.trim().trim_end_matches('#').trim();
            if text.is_empty() {
                return Some(String::new());
            }
            return Some(if in_callout {
                format!("**{}**", text.trim_matches('*'))
            } else {
                format!("{} {text}", "#".repeat((level + 2).min(6)))
            });
        }
    }
    Some(line.to_string())
}

struct Tag<'a> {
    name: String,
    closing: bool,
    attrs: &'a str,
    len: usize,
}

fn parse_tag(s: &str) -> Option<Tag<'_>> {
    let rest = s.strip_prefix('<')?;
    let (closing, rest) = match rest.strip_prefix('/') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    if !KNOWN_TAGS.contains(&name.as_str()) {
        return None;
    }
    let after = &rest[name.len()..];
    if !(after.starts_with('>') || after.starts_with('/') || after.starts_with(char::is_whitespace))
    {
        return None;
    }
    let end = after.find('>')?;
    if after[..end].contains('<') {
        return None;
    }
    let len = 1 + closing as usize + name.len() + end + 1;
    Some(Tag {
        name,
        closing,
        attrs: &after[..end],
        len,
    })
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let i = from + pos;
        from = i + name.len();
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        let rest = attrs[i + name.len()..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or(""),
        };
        return Some(value.to_string());
    }
    None
}

fn html_to_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bold = 0usize;
    let mut links: Vec<Option<String>> = vec![];
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        // Inline code: copy verbatim.
        if rest.starts_with('`') {
            let ticks = rest.chars().take_while(|c| *c == '`').count();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                let len = ticks + end + ticks;
                out.push_str(&rest[..len]);
                i += len;
                continue;
            }
        }
        if rest.starts_with('<') {
            if let Some(tag) = parse_tag(rest) {
                emit(&tag, &mut out, &mut bold, &mut links);
                i += tag.len;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    if bold > 0 {
        out.push_str("**");
    }
    out
}

fn emit(tag: &Tag, out: &mut String, bold: &mut usize, links: &mut Vec<Option<String>>) {
    let n = tag.name.as_str();
    match (n, tag.closing) {
        ("strong" | "b" | "summary" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "dt", false) => {
            if n != "strong" && n != "b" && !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            if *bold == 0 {
                out.push_str("**");
            }
            *bold += 1;
        }
        ("strong" | "b" | "summary" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "dt", true) => {
            if *bold > 0 {
                *bold -= 1;
                if *bold == 0 {
                    // "**text **" does not render as bold
                    let trimmed = out.trim_end().len();
                    out.truncate(trimmed);
                    out.push_str("**");
                }
            }
            if n != "strong" && n != "b" {
                out.push_str("\n\n");
            }
        }
        ("em" | "i", _) => out.push('*'),
        ("code" | "kbd" | "tt" | "samp", _) => out.push('`'),
        ("del" | "s" | "strike", _) => out.push_str("~~"),
        ("br", _) => out.push('\n'),
        ("hr", _) => out.push_str("\n\n---\n\n"),
        ("p" | "div" | "details" | "table" | "pre" | "blockquote" | "dl", _) => {
            if !out.ends_with("\n\n") && !out.is_empty() {
                out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
            }
        }
        ("li", false) => {
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            out.push_str("- ");
        }
        ("ul" | "ol" | "tr" | "dd", true) | ("li", true) => {
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        ("td" | "th", true) => out.push(' '),
        ("a", false) => {
            let href = attr(tag.attrs, "href").filter(|h| !h.is_empty() && h != "#");
            if href.is_some() {
                out.push('[');
            }
            links.push(href);
        }
        ("a", true) => {
            if let Some(Some(href)) = links.pop() {
                if out.ends_with('[') {
                    out.pop(); // empty link text: drop the link
                } else {
                    out.push_str(&format!("]({href})"));
                }
            }
        }
        ("img", _) => {
            if let Some(alt) = attr(tag.attrs, "alt").filter(|a| !a.trim().is_empty()) {
                out.push_str(alt.trim());
            }
        }
        _ => {} // picture, source, span, sub, sup, center, ... : drop the tag, keep the text
    }
}

/// `![alt](url)` -> `alt`; then drop links left without text and `[text](#)` -> `text`.
fn remove_images(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("![") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after
            .find("](")
            .and_then(|mid| after[mid + 2..].find(')').map(|end| (mid, mid + 2 + end)))
        {
            Some((mid, end)) if !after[..mid].contains('\n') => {
                out.push_str(after[..mid].trim());
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("![");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    // Empty links: `[](...)`
    let mut s = out;
    while let Some(pos) = s.find("[](") {
        match s[pos..].find(')') {
            Some(end) => s.replace_range(pos..pos + end + 1, ""),
            None => break,
        }
    }
    s.replace("](#)", "]").replace("[ ]", "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codescene() {
        let body = "[//]: # (cs-code-health)\n\n**Gates Failed**\n[![](https://x/x1.svg)](#) Enforce rules `(1 file)`\n\n<details><summary>Reason for failure</summary>\n\n| a | b |\n|-|-|\n| c | d |\n</details>\n\n####   \u{a0}  \n**Profile:** [Bare](https://x)";
        let out = github_markdown(body, true);
        assert_eq!(
            out,
            "**Gates Failed**\nEnforce rules `(1 file)`\n\n**Reason for failure**\n\n| a | b |\n|-|-|\n| c | d |\n\n**Profile:** [Bare](https://x)"
        );
    }

    #[test]
    fn copilot() {
        let body = "##### 🔵 Needs a closer look\n\n<details>\n<summary><strong>1 resolved since last review</strong></summary>\n\n- <picture><source srcset=\"a.svg\"><img src=\"a.png\" alt=\"Medium severity\" width=\"62\"></picture> [Invert it](https://x)\n</details>";
        let out = github_markdown(body, true);
        assert_eq!(
            out,
            "**🔵 Needs a closer look**\n\n**1 resolved since last review**\n\n- Medium severity [Invert it](https://x)"
        );
    }

    #[test]
    fn leaves_code_and_generics_alone() {
        let body = "Use `Vec<b>` and Option<String>\n```html\n<b>x</b>\n```";
        assert_eq!(github_markdown(body, false), body);
        assert_eq!(github_markdown("# Title\ntext", false), "### Title\ntext");
        assert_eq!(
            github_markdown("<a href=\"https://u\">link</a><br>next", true),
            "[link](https://u)\nnext"
        );
    }

    #[test]
    fn suggestion() {
        assert_eq!(
            github_markdown("```suggestion\nfoo\n```", true),
            "**Suggested change:**\n```\nfoo\n```"
        );
    }
}
