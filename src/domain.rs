use crate::{
    api::{self, string, Api, Error},
    blocks,
};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    pub icon: String,
}

impl Item {
    pub fn parse(raw: &Value) -> Option<Self> {
        let kind = string(raw, "object");
        if !matches!(kind, "page" | "data_source" | "database")
            || raw["in_trash"] == true
            || raw["archived"] == true
        {
            return None;
        }
        let id = api::id(string(raw, "id"))?;
        let title = match kind {
            "page" => raw["properties"]
                .as_object()
                .into_iter()
                .flat_map(|p| p.values())
                .find(|p| p["type"] == "title")
                .map(|p| blocks::rich_text(&p["title"]))
                .unwrap_or_default(),
            _ => blocks::rich_text(&raw["title"]),
        };
        let destination_id = if kind == "data_source" {
            api::id(string(&raw["parent"], "database_id")).unwrap_or_else(|| id.clone())
        } else {
            id.clone()
        };
        let fallback_url = format!("https://www.notion.so/{}", destination_id.replace('-', ""));
        let url = blocks::url(string(raw, "url"))
            .filter(|u| u.scheme() == "https" && u.host_str().is_some_and(notion_host))
            .map(|u| u.to_string())
            .unwrap_or(fallback_url);
        Some(Self {
            id,
            kind: kind.into(),
            title: if title.trim().is_empty() {
                "Untitled".into()
            } else {
                title.chars().take(300).collect()
            },
            url,
            icon: string(&raw["icon"], "emoji").chars().take(12).collect(),
        })
    }

    pub fn source_choice(raw: &Value) -> Option<Self> {
        Self::parse(
            &json!({"object":"data_source","id":raw["id"],"title":blocks::rich(string(raw,"name"))}),
        )
    }

    pub fn is_page(&self) -> bool {
        self.kind == "page"
    }
}

pub fn notion_host(host: &str) -> bool {
    ["notion.so", "notion.com", "notion.site"]
        .iter()
        .any(|base| host == *base || host.ends_with(&format!(".{base}")))
}

pub fn page_id_from_url(raw: &str) -> Option<String> {
    let url = blocks::url(raw)?;
    if !url.host_str().is_some_and(notion_host) {
        return None;
    }
    let last = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .next_back()?;
    api::id(last).or_else(|| last.get(last.len().checked_sub(32)?..).and_then(api::id))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Property {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub options: Vec<(String, String)>,
}

impl Property {
    pub fn field(&self) -> String {
        format!("property::{}::{}", self.kind, self.id)
    }

    fn from_schema(name: &str, raw: &Value) -> Option<Self> {
        let kind = string(raw, "type");
        let id = string(raw, "id");
        if id.is_empty()
            || !matches!(
                kind,
                "title"
                    | "rich_text"
                    | "number"
                    | "date"
                    | "select"
                    | "status"
                    | "multi_select"
                    | "relation"
                    | "people"
                    | "checkbox"
                    | "url"
                    | "email"
                    | "phone_number"
            )
        {
            return None;
        }
        let options = raw[kind]["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| Some((o["id"].as_str()?.to_owned(), o["name"].as_str()?.to_owned())))
            .collect();
        Some(Self {
            id: id.into(),
            title: name.into(),
            kind: kind.into(),
            options,
        })
    }

    pub fn from_value(&self, raw: &Value) -> Value {
        let value = &raw[&self.kind];
        match self.kind.as_str() {
            "title" | "rich_text" => json!(blocks::rich_text(value)),
            "number" => json!(if value.is_number() {
                value.to_string()
            } else {
                String::new()
            }),
            "select" | "status" => json!(string(value, "id")),
            "multi_select" => json!(value
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| string(v, "id"))
                .collect::<Vec<_>>()),
            "relation" | "people" => json!(value
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| string(v, "id"))
                .collect::<Vec<_>>()
                .join(", ")),
            "date" => {
                if let Some(end) = value["end"].as_str() {
                    json!(format!("{} / {end}", string(value, "start")))
                } else {
                    json!(string(value, "start"))
                }
            }
            "checkbox" => json!(value.as_bool().unwrap_or(false)),
            _ => json!(value.as_str().unwrap_or_default()),
        }
    }

    pub fn format(&self, raw: &Value, editing: bool) -> Result<Option<Value>, String> {
        let text = raw.as_str().unwrap_or_default().trim();
        let kind = self.kind.as_str();
        let invalid = || format!("Enter a valid value for {}.", self.title);
        let data = match kind {
            "title" | "rich_text" => {
                if kind == "title" && text.is_empty() {
                    return Err(format!("Enter a title for {}.", self.title));
                }
                let runs = blocks::rich(text);
                if runs.len() > 100 {
                    return Err(invalid());
                }
                json!(runs)
            }
            "checkbox" => match raw.as_bool() {
                Some(value) => json!(value),
                None if raw.is_null() => return Ok(None),
                None => return Err(invalid()),
            },
            "number" if !text.is_empty() || raw.is_number() => {
                let value = raw
                    .as_f64()
                    .or_else(|| text.parse().ok())
                    .and_then(serde_json::Number::from_f64)
                    .ok_or_else(invalid)?;
                Value::Number(value)
            }
            "date" if !text.is_empty() => {
                let (start, end) = text.split_once(" / ").map_or((text, None), |(start, end)| {
                    (start.trim(), Some(end.trim()))
                });
                let valid = |v: &str| {
                    NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok()
                        || DateTime::parse_from_rfc3339(v).is_ok()
                };
                if !valid(start) || end.is_some_and(|v| !valid(v)) {
                    return Err(invalid());
                }
                json!({"start":start,"end":end})
            }
            "select" | "status" if !text.is_empty() => {
                if !self.options.iter().any(|(id, _)| id == text) {
                    return Err(invalid());
                }
                json!({"id":text})
            }
            "multi_select" => {
                let items = raw.as_array().map(Vec::as_slice).unwrap_or(&[]);
                if !raw.is_null() && !raw.is_array() {
                    return Err(invalid());
                }
                if items.len() > 100 {
                    return Err(invalid());
                }
                let values = items
                    .iter()
                    .map(|v| {
                        let id = v.as_str().ok_or_else(invalid)?;
                        if !self.options.iter().any(|(value, _)| value == id) {
                            return Err(invalid());
                        }
                        Ok(json!({"id":id}))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                json!(values)
            }
            "relation" | "people" => {
                let ids = text
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .map(|s| api::id(s).ok_or_else(invalid))
                    .collect::<Result<Vec<_>, _>>()?;
                if ids.len() > 100 {
                    return Err(invalid());
                }
                json!(ids.iter().map(|id| json!({"id":id})).collect::<Vec<_>>())
            }
            "url" if !text.is_empty() => json!(blocks::url(text).ok_or_else(invalid)?.as_str()),
            "email" | "phone_number" if !text.is_empty() => {
                if text.len() > 200 {
                    return Err(invalid());
                }
                json!(text)
            }
            _ if editing => Value::Null,
            _ => return Ok(None),
        };
        if !editing && (data.is_null() || data.as_array().is_some_and(|a| a.is_empty())) {
            return Ok(None);
        }
        Ok(Some(json!({kind:data})))
    }
}

pub fn schema(data: &Value) -> Vec<Property> {
    let mut out: Vec<_> = data["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| Property::from_schema(name, value))
        .collect();
    out.sort_by_key(|prop| prop.kind != "title");
    out
}

pub fn title_property(properties: &[Property]) -> Result<&Property, String> {
    properties
        .iter()
        .find(|p| p.kind == "title")
        .ok_or_else(|| "The Notion source did not return a title property.".into())
}

pub fn properties(props: &[Property], values: &Value, editing: bool) -> Result<Value, String> {
    let mut out = Map::new();
    for prop in props {
        if let Some(value) = prop.format(&values[&prop.field()], editing)? {
            out.insert(prop.id.clone(), value);
        }
    }
    Ok(Value::Object(out))
}

pub fn metadata(raw: &Value) -> Vec<(String, String)> {
    let mut values = Vec::new();
    for (caption, key) in [
        ("Last edited", "last_edited_time"),
        ("Created", "created_time"),
    ] {
        let timestamp = string(raw, key);
        if timestamp.len() >= 10 {
            values.push((caption.into(), timestamp.chars().take(24).collect()));
        }
    }
    for (name, prop) in raw["properties"].as_object().into_iter().flatten() {
        let kind = string(prop, "type");
        let data = &prop[kind];
        let value = match kind {
            "title" => continue,
            "rich_text" => blocks::rich_text(data),
            "date" => string(data, "start").to_owned(),
            "select" | "status" => string(data, "name").to_owned(),
            "formula" | "rollup" => {
                let value = &data[string(data, "type")];
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            }
            "multi_select" | "people" | "relation" | "files" => data
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| {
                    v["name"]
                        .as_str()
                        .or_else(|| v["id"].as_str())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(", "),
            _ => data.as_str().map(str::to_owned).unwrap_or_else(|| {
                if data.is_number() || data.is_boolean() {
                    data.to_string()
                } else {
                    String::new()
                }
            }),
        };
        if !value.trim().is_empty() {
            values.push((name.clone(), value.chars().take(500).collect()));
        }
        if values.len() >= 50 {
            break;
        }
    }
    values
}

pub async fn preview(api: &Api<'_>, item: &Item) -> Result<String, Error> {
    enum Work {
        List(String, Option<String>, usize),
        Render(Value, usize),
    }

    const PREVIEW_LIMIT: usize = 120_000;
    const TRUNCATED: &str =
        "\n\n*Preview limit reached. Open in Notion to see the remaining content.*";
    let mut pending = vec![Work::List(item.id.clone(), None, 0)];
    let mut seen = HashSet::new();
    let mut requests = 0;
    let mut output = String::new();
    while let Some(work) = pending.pop() {
        match work {
            Work::List(id, cursor, level) => {
                if requests >= 18 || !seen.insert((id.clone(), cursor.clone())) {
                    output.push_str(TRUNCATED);
                    break;
                }
                requests += 1;
                let batch = api.children(&id, cursor.as_deref()).await?;
                if let Some(cursor) = batch.cursor {
                    pending.push(Work::List(id, Some(cursor), level));
                }
                for block in batch.rows.into_iter().rev() {
                    pending.push(Work::Render(block, level));
                }
            }
            Work::Render(block, level) => {
                let text = blocks::preview(&block);
                if output.len() + text.len() > PREVIEW_LIMIT {
                    output.push_str(TRUNCATED);
                    break;
                }
                if !text.is_empty() {
                    output.push_str(&format!("{}{}\n\n", "  ".repeat(level), text));
                }
                if block["has_children"] == true
                    && level < 3
                    && !matches!(string(&block, "type"), "child_page" | "child_database")
                {
                    if let Some(id) = api::id(string(&block, "id")) {
                        pending.push(Work::List(id, None, level + 1));
                    }
                }
            }
        }
    }
    if output.trim().is_empty() {
        output.push_str("*Page is empty.*");
    }
    Ok(output)
}

fn equal_title(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

const NOTES_LOOKUP_LIMIT: usize = 18;

async fn exact_page(api: &Api<'_>, title: &str) -> Result<Option<Item>, String> {
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    for _ in 0..NOTES_LOOKUP_LIMIT {
        let batch = api.search(title, Some("page"), cursor.as_deref()).await?;
        if let Some(item) = batch
            .rows
            .iter()
            .filter(|row| {
                !matches!(
                    string(&row["parent"], "type"),
                    "database_id" | "data_source_id"
                )
            })
            .filter_map(Item::parse)
            .find(|item| equal_title(&item.title, title))
        {
            return Ok(Some(item));
        }
        cursor = batch.cursor;
        match &cursor {
            None => return Ok(None),
            Some(value) if !seen.insert(value.clone()) => break,
            _ => {}
        }
    }
    Err(
        "The notes search did not finish. Refine the notes page name or use an explicit parent."
            .into(),
    )
}

async fn child_page(api: &Api<'_>, parent: &Item, title: &str) -> Result<Option<Item>, String> {
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    for _ in 0..NOTES_LOOKUP_LIMIT {
        let batch = api.children(&parent.id, cursor.as_deref()).await?;
        for row in batch.rows {
            if row["type"] == "child_page"
                && equal_title(string(&row["child_page"], "title"), title)
            {
                let page_id =
                    api::id(string(&row, "id")).ok_or("Notion returned an invalid child page.")?;
                match api.page(&page_id).await {
                    Ok(raw) => {
                        if let Some(item) = Item::parse(&raw) {
                            return Ok(Some(item));
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        cursor = batch.cursor;
        match &cursor {
            None => return Ok(None),
            Some(value) if !seen.insert(value.clone()) => break,
            _ => {}
        }
    }
    Err("The notes page has too many children to verify a unique daily page.".into())
}

async fn create_child(
    api: &Api<'_>,
    parent: &Item,
    title: &str,
    content: Vec<Value>,
) -> Result<Item, String> {
    let props = json!({"title":{"type":"title","title":blocks::rich(title)}});
    let response = api.create(&parent.id, false, props, content).await?;
    Item::parse(&response).ok_or_else(|| {
        "Notion did not return the created page. Verify the destination before trying again.".into()
    })
}

pub async fn note(
    api: &Api<'_>,
    root_name: &str,
    date: &str,
    parent: Option<&Item>,
    content: Vec<Value>,
) -> Result<Item, String> {
    let root = match parent {
        Some(parent) => match child_page(api, parent, root_name).await? {
            Some(root) => root,
            None => create_child(api, parent, root_name, vec![]).await?,
        },
        None => exact_page(api, root_name).await?.ok_or_else(|| {
            format!("No {root_name} page was found. Choose an accessible parent page first.")
        })?,
    };
    match child_page(api, &root, date).await? {
        Some(page) => {
            api.append(&page.id, content, false).await?;
            Ok(page)
        }
        None => create_child(api, &root, date, content).await,
    }
}

pub fn date_title(raw: &str) -> String {
    let now = Local::now();
    let mut out = String::new();
    let mut literal = false;
    let mut pattern = raw.chars().peekable();
    while let Some(c) = pattern.next() {
        if c == '\'' {
            if pattern.peek() == Some(&'\'') {
                pattern.next();
                out.push('\'');
            } else {
                literal = !literal;
            }
        } else if literal || !c.is_ascii_alphabetic() {
            out.push(c);
        } else {
            let mut count = 1;
            while pattern.peek() == Some(&c) {
                pattern.next();
                count += 1;
            }
            let spec = match (c, count) {
                ('y', 1 | 4) => "%Y",
                ('y', 2) => "%y",
                ('M' | 'L', 1) => "%-m",
                ('M' | 'L', 2) => "%m",
                ('M' | 'L', 3) => "%b",
                ('M' | 'L', 4) => "%B",
                ('d', 1) => "%-d",
                ('d', 2) => "%d",
                ('E', 1..=3) => "%a",
                ('E', 4) => "%A",
                ('H', 1) => "%-H",
                ('H', 2) => "%H",
                ('h', 1) => "%-I",
                ('h', 2) => "%I",
                ('m', 1) => "%-M",
                ('m', 2) => "%M",
                ('s', 1) => "%-S",
                ('s', 2) => "%S",
                ('a', 1..=4) => "%p",
                _ => return now.format("%Y-%m-%d").to_string(),
            };
            out.push_str(&now.format(spec).to_string());
        }
    }
    if literal || out.trim().is_empty() {
        now.format("%Y-%m-%d").to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_schema_and_uses_property_ids() {
        let fields = schema(&json!({"properties":{
            "Renamed":{"id":"title","type":"title"},"Points":{"id":"num","type":"number"},
            "State":{"id":"opt","type":"select","select":{"options":[{"id":"selected","name":"One"}]}},
            "Computed":{"id":"read-only","type":"formula"}}}));
        let good=properties(&fields,&json!({"property::title::title":"Example","property::number::num":"0","property::select::opt":"selected"}),false).unwrap();
        assert_eq!(good["num"]["number"], 0.0);
        assert_eq!(good["opt"]["select"]["id"], "selected");
        assert!(properties(&fields, &json!({"property::select::opt":"alien"}), false).is_err());
        assert_eq!(fields.len(), 3);
    }
    #[test]
    fn note_date_has_fallback_and_notion_links_validate() {
        assert!(NaiveDate::parse_from_str(&date_title("invalid"), "%Y-%m-%d").is_ok());
        assert!(date_title("dd MMM yyyy 'notes'").ends_with(" notes"));
        assert!(
            page_id_from_url("https://evil.example/Some-da000000000000000000000000000001")
                .is_none()
        );
        assert_eq!(
            page_id_from_url("https://notion.so/Some-da000000000000000000000000000001").unwrap(),
            "da000000-0000-0000-0000-000000000001"
        );
    }
}
