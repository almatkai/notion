use crate::api::string;
use chrono::Local;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{json, Value};

pub const MAX_BLOCKS: usize = 100;

pub fn rich(raw: &str) -> Vec<Value> {
    rich_run(raw, &Inline::default())
}

#[derive(Clone, Default)]
struct Inline {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
    link: Option<String>,
}

fn rich_run(raw: &str, style: &Inline) -> Vec<Value> {
    let mut chunks = vec![];
    let mut start = 0;
    let mut len = 0;
    for (offset, c) in raw.char_indices() {
        if len + c.len_utf16() > 2000 {
            chunks.push(&raw[start..offset]);
            start = offset;
            len = 0;
        }
        len += c.len_utf16();
    }
    if start < raw.len() {
        chunks.push(&raw[start..]);
    }
    chunks
        .into_iter()
        .map(|text| {
            let mut item = json!({"type":"text","text":{"content":text}});
            if let Some(url) = &style.link {
                item["text"]["link"] = json!({"url":url});
            }
            if style.bold || style.italic || style.strikethrough || style.code {
                item["annotations"] = json!({"bold":style.bold,"italic":style.italic,
                "strikethrough":style.strikethrough,"code":style.code});
            }
            item
        })
        .collect()
}

pub fn rich_text(value: &Value) -> String {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            r["plain_text"]
                .as_str()
                .or_else(|| r["text"]["content"].as_str())
                .or_else(|| r["equation"]["expression"].as_str())
                .unwrap_or_default()
        })
        .collect()
}

pub fn block(kind: &str, pieces: Vec<Value>) -> Value {
    let mut properties = json!({"rich_text":pieces});
    if kind == "to_do" {
        properties["checked"] = json!(false);
    }
    json!({"object":"block","type":kind,kind:properties})
}

pub fn date_divider(children: &mut Vec<Value>) {
    let date = Local::now().format("%Y-%m-%d").to_string();
    children.splice(
        0..0,
        [
            json!({"object":"block","type":"divider","divider":{}}),
            block(
                "paragraph",
                vec![json!({"type":"mention","mention":{"type":"date","date":{"start":date}}})],
            ),
        ],
    );
}

pub fn bookmark(url: &str) -> Value {
    json!({"object":"block","type":"bookmark","bookmark":{"url":url}})
}

pub fn linked_title(title: &str, url: &str) -> Value {
    block(
        "paragraph",
        rich_run(
            title,
            &Inline {
                link: Some(url.into()),
                ..Inline::default()
            },
        ),
    )
}

pub fn plain(raw: &str) -> Vec<Value> {
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| block("paragraph", rich(line)))
        .collect()
}

struct Frame {
    kind: &'static str,
    text: Vec<Value>,
    children: Vec<Value>,
    checked: Option<bool>,
    language: &'static str,
}

impl Frame {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            text: vec![],
            children: vec![],
            checked: None,
            language: "plain text",
        }
    }

    fn append(&mut self, child: Value) {
        if self.kind != "root"
            && self.text.is_empty()
            && child["type"] == "paragraph"
            && self.children.is_empty()
        {
            self.text = child["paragraph"]["rich_text"]
                .as_array()
                .cloned()
                .unwrap_or_default();
        } else {
            self.children.push(child);
        }
    }

    fn finish(self) -> Value {
        let kind = if self.checked.is_some() {
            "to_do"
        } else {
            self.kind
        };
        let mut item = block(kind, self.text);
        if let Some(checked) = self.checked {
            item[kind]["checked"] = json!(checked);
        }
        if kind == "code" {
            item[kind]["language"] = json!(self.language);
        }
        if !self.children.is_empty() {
            item[kind]["children"] = json!(self.children);
        }
        item
    }
}

fn heading(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "heading_1",
        HeadingLevel::H2 => "heading_2",
        _ => "heading_3",
    }
}

fn language(info: &str) -> &'static str {
    match info
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "rust" | "rs" => "rust",
        "js" | "javascript" => "javascript",
        "tsx" | "ts" | "typescript" => "typescript",
        "python" | "py" => "python",
        "bash" => "bash",
        "shell" | "sh" => "shell",
        "powershell" | "ps1" => "powershell",
        "html" => "html",
        "css" => "css",
        "json" => "json",
        "sql" => "sql",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "go" => "go",
        "java" => "java",
        "c" => "c",
        "cpp" | "c++" => "c++",
        "cs" | "csharp" => "c#",
        "swift" => "swift",
        "diff" => "diff",
        "markdown" | "md" => "markdown",
        _ => "plain text",
    }
}

pub fn url(raw: &str) -> Option<reqwest::Url> {
    reqwest::Url::parse(raw).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.as_str().len() <= 2000
    })
}

pub fn markdown(raw: &str) -> Vec<Value> {
    let mut frames = vec![Frame::new("root")];
    let mut inline = vec![Inline::default()];
    let mut lists = vec![];
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(raw, options) {
        match event {
            Event::Start(tag) => {
                let frame = match tag {
                    Tag::Paragraph => Some(Frame::new("paragraph")),
                    Tag::Heading { level, .. } => Some(Frame::new(heading(level))),
                    Tag::BlockQuote(_) => Some(Frame::new("quote")),
                    Tag::CodeBlock(info) => {
                        let mut frame = Frame::new("code");
                        if let CodeBlockKind::Fenced(info) = info {
                            frame.language = language(&info);
                        }
                        Some(frame)
                    }
                    Tag::List(start) => {
                        lists.push(start.is_some());
                        None
                    }
                    Tag::Item => Some(Frame::new(if lists.last() == Some(&true) {
                        "numbered_list_item"
                    } else {
                        "bulleted_list_item"
                    })),
                    Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. } => {
                        let mut style = inline.last().cloned().unwrap_or_default();
                        match tag {
                            Tag::Emphasis => style.italic = true,
                            Tag::Strong => style.bold = true,
                            Tag::Strikethrough => style.strikethrough = true,
                            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                                style.link = url(&dest_url).map(|u| u.to_string());
                            }
                            _ => {}
                        }
                        inline.push(style);
                        None
                    }
                    _ => None,
                };
                if let Some(frame) = frame {
                    frames.push(frame);
                }
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock
                | TagEnd::Item,
            ) => {
                if frames.len() > 1 {
                    let frame = frames.pop().unwrap().finish();
                    frames.last_mut().unwrap().append(frame);
                }
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::End(
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image,
            ) => {
                inline.pop();
            }
            Event::TaskListMarker(checked) => {
                if let Some(frame) = frames
                    .iter_mut()
                    .rev()
                    .find(|f| matches!(f.kind, "bulleted_list_item" | "numbered_list_item"))
                {
                    frame.checked = Some(checked);
                }
            }
            Event::Rule => frames
                .last_mut()
                .unwrap()
                .append(json!({"object":"block","type":"divider","divider":{}})),
            Event::Code(text) => {
                let mut style = inline.last().cloned().unwrap_or_default();
                style.code = true;
                frames
                    .last_mut()
                    .unwrap()
                    .text
                    .extend(rich_run(&text, &style));
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                let style = inline.last().cloned().unwrap_or_default();
                frames
                    .last_mut()
                    .unwrap()
                    .text
                    .extend(rich_run(&text, &style));
            }
            Event::SoftBreak | Event::HardBreak => {
                frames.last_mut().unwrap().text.extend(rich("\n"))
            }
            _ => {}
        }
    }
    let mut root = frames.pop().expect("root frame");
    if !root.text.is_empty() {
        root.children.push(block("paragraph", root.text));
    }
    root.children
}

/// Reject a write up front when it would require partial, multi-request mutation.
pub fn validate(children: &[Value]) -> Result<(), String> {
    fn item_ok(block: &Value, level: usize) -> bool {
        let v = &block[string(block, "type")];
        v["rich_text"].as_array().is_none_or(|r| r.len() <= 100)
            && v["children"].as_array().is_none_or(|children| {
                level < 2
                    && children.len() <= MAX_BLOCKS
                    && children.iter().all(|c| item_ok(c, level + 1))
            })
    }
    if children.len() > MAX_BLOCKS || !children.iter().all(|v| item_ok(v, 1)) {
        return Err("Notion limits one write to 100 blocks per level, two nesting levels and 100 styled text runs per block. Split this content and try again.".into());
    }
    Ok(())
}

fn safe_markdown(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if "\\`*_{}[]<>&#~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn preview_rich(rich: &Value) -> String {
    rich.as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let text = r["plain_text"]
                .as_str()
                .or_else(|| r["text"]["content"].as_str())
                .unwrap_or_default();
            let mut text = safe_markdown(text);
            let flags = &r["annotations"];
            for (key, mark) in [
                ("code", "`"),
                ("bold", "**"),
                ("italic", "*"),
                ("strikethrough", "~~"),
            ] {
                if flags[key] == true {
                    text = format!("{mark}{text}{mark}");
                }
            }
            if let Some(link) = r["href"]
                .as_str()
                .or_else(|| r["text"]["link"]["url"].as_str())
                .and_then(url)
            {
                text = format!("[{text}](<{link}>)");
            }
            text
        })
        .collect()
}

pub fn preview(item: &Value) -> String {
    let kind = string(item, "type");
    let data = &item[kind];
    let text = preview_rich(&data["rich_text"]);
    match kind {
        "heading_1" => format!("# {text}"),
        "heading_2" => format!("## {text}"),
        "heading_3" | "heading_4" => format!("### {text}"),
        "bulleted_list_item" => format!("- {text}"),
        "numbered_list_item" => format!("1. {text}"),
        "to_do" => format!(
            "- [{}] {text}",
            if data["checked"] == true { "x" } else { " " }
        ),
        "quote" | "callout" => format!("> {text}"),
        "code" => format!(
            "````\n{}\n````",
            rich_text(&data["rich_text"]).replace("````", "```\u{200b}`")
        ),
        "divider" => "---".into(),
        "child_page" | "child_database" => format!("**{}**", safe_markdown(string(data, "title"))),
        "bookmark" | "embed" | "link_preview" => url(string(data, "url"))
            .map(|u| format!("[{u}](<{u}>)"))
            .unwrap_or_default(),
        "image" | "video" | "pdf" | "audio" | "file" => {
            let file = &data[string(data, "type")];
            url(string(file, "url"))
                .map(|u| format!("[{}](<{u}>)", safe_markdown(kind)))
                .unwrap_or_default()
        }
        "table_row" => data["cells"]
            .as_array()
            .into_iter()
            .flatten()
            .map(preview_rich)
            .collect::<Vec<_>>()
            .join(" | "),
        "equation" => format!("`{}`", safe_markdown(string(data, "expression"))),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn produces_annotated_blocks_and_nested_lists() {
        let rows = markdown("# Runbook\n\nA **bold** [link](https://example.com)\n\n- [x] Done\n  - Child\n\n```rs\nlet num = 1;\n```");
        assert_eq!(rows[0]["type"], "heading_1");
        assert_eq!(
            rows[1]["paragraph"]["rich_text"][1]["annotations"]["bold"],
            true
        );
        assert_eq!(
            rows[1]["paragraph"]["rich_text"][3]["text"]["link"]["url"],
            "https://example.com/"
        );
        assert_eq!(rows[2]["to_do"]["checked"], true);
        assert_eq!(
            rows[2]["to_do"]["children"][0]["type"],
            "bulleted_list_item"
        );
        assert_eq!(rows[3]["code"]["language"], "rust");
        assert!(validate(&rows).is_ok());
    }
    #[test]
    fn chunks_unicode_without_breaking_codepoints() {
        let source = "??".repeat(2500);
        let result = rich(&source);
        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|r| r["text"]["content"]
            .as_str()
            .unwrap()
            .encode_utf16()
            .count()
            <= 2000));
        assert_eq!(rich_text(&json!(result)), source);
    }
}
