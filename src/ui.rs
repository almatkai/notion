use crate::{
    api::string,
    domain::{self, Item, Property},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tezbar_extension_sdk::view;

pub const LIST_LIMIT: usize = 250;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Purpose {
    Search,
    Create,
    Capture,
    Append,
    NoteParent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ListKind {
    Workspace(Purpose),
    DatabaseSources(Purpose, Item),
    Records(Item, String),
    Properties(Item, Vec<Property>, Value),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct List {
    pub kind: ListKind,
    pub query: String,
    pub rows: Vec<Item>,
    pub cursor: Option<String>,
    pub seen_cursors: Vec<String>,
}

impl List {
    pub fn new(kind: ListKind) -> Self {
        Self {
            kind,
            query: String::new(),
            rows: vec![],
            cursor: None,
            seen_cursors: vec![],
        }
    }
    pub fn title(&self) -> &str {
        match &self.kind {
            ListKind::Workspace(Purpose::Search) => "Search Notion",
            ListKind::Workspace(Purpose::Create) => "Choose a Database",
            ListKind::Workspace(Purpose::Capture) => "Choose a Capture Destination",
            ListKind::Workspace(Purpose::Append) => "Choose a Notion Page",
            ListKind::Workspace(Purpose::NoteParent) => "Choose a Parent Page",
            ListKind::DatabaseSources(_, _) => "Choose a Data Source",
            ListKind::Records(item, _) => &item.title,
            ListKind::Properties(..) => "Edit Page Properties",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Form {
    Lookup(Purpose),
    Create(Item, Vec<Property>),
    Append(Item, bool),
    Note(Option<Item>),
    Capture(Item),
    Edit(Item, Property),
}

impl Form {
    pub fn title(&self) -> &str {
        match self {
            Self::Lookup(_) => "Find by Notion URL or ID",
            Self::Create(..) => "Create Database Page",
            Self::Append(_, true) => "Add Text to Page",
            Self::Append(_, false) => "Append Content to Page",
            Self::Note(_) => "Add Note",
            Self::Capture(_) => "Quick Capture",
            Self::Edit(..) => "Edit Property",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Screen {
    Setup,
    Connection(String),
    List(List),
    Form(Form, Value),
    Preview(Item, String, Vec<(String, String)>, bool),
    Saved(Item, String),
    Trashed(Item),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    Settings,
    Access,
    TestConnection,
    Back,
    Refresh,
    Lookup(Purpose),
    Choose(Purpose, Item),
    Navigate(Item),
    Open(Item, bool),
    Copy(Item, CopyKind),
    QuickLink(Item),
    Pin(Item, bool),
    Trash(Item),
    Restore(Item),
    Append(Item),
    Create(Item),
    Edit(Item),
    EditField(Item, Property, Value),
    ToggleMetadata,
    NoteParent,
    ClearNoteParent,
    UseClipboard,
    UseSelection,
    Submit(bool),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CopyKind {
    Url,
    Title,
    Formatted,
    Paste,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Markers {
    pub pinned: Vec<String>,
    pub recent: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub screen: Screen,
    pub history: Vec<Screen>,
    pub actions: BTreeMap<String, Action>,
    pub revision: usize,
    pub notice: String,
    pub markers: Markers,
    pub argument_text: String,
}

impl Session {
    pub fn new(screen: Screen) -> Self {
        Self {
            screen,
            history: vec![],
            actions: BTreeMap::new(),
            revision: 0,
            notice: String::new(),
            markers: Markers::default(),
            argument_text: String::new(),
        }
    }
    pub fn push(&mut self, screen: Screen) -> Result<(), String> {
        if self.history.len() >= 12 {
            return Err("Close an open view before navigating deeper.".into());
        }
        self.history
            .push(std::mem::replace(&mut self.screen, screen));
        Ok(())
    }
    pub fn back(&mut self) {
        if let Some(screen) = self.history.pop() {
            self.screen = screen;
        }
    }
}

#[derive(Default)]
struct Actions {
    revision: usize,
    descriptors: Vec<Value>,
    handlers: BTreeMap<String, Action>,
}

impl Actions {
    fn add(&mut self, title: &str, action: Action) -> String {
        let id = format!("notion-{}-{}", self.revision, self.descriptors.len());
        let kind = if matches!(action, Action::Submit(_)) {
            "submit-form"
        } else {
            "callback"
        };
        let mut descriptor = json!({"id":id,"title":title,"kind":kind});
        if matches!(action, Action::Trash(_)) {
            descriptor["style"] = json!("destructive");
        }
        self.descriptors.push(descriptor);
        self.handlers.insert(id.clone(), action);
        id
    }
}

fn description(title: &str, text: &str) -> Value {
    json!({"type":"Form.Description","props":{"title":title,"text":text}})
}

fn field(kind: &str, id: &str, title: &str, value: &Value) -> Value {
    json!({"type":format!("Form.{kind}"),"props":{"id":id,"title":title,"value":value}})
}

fn choice_field(
    kind: &str,
    id: &str,
    title: &str,
    value: &Value,
    choices: impl IntoIterator<Item = (String, String)>,
) -> Value {
    let mut field = field(kind, id, title, value);
    let children=choices.into_iter().map(|(value,title)|json!({"type":format!("Form.{kind}.Item"),"props":{"value":value,"title":title}})).collect::<Vec<_>>();
    field["children"] = json!(children);
    field
}

fn checkbox(id: &str, label: &str, value: &Value) -> Value {
    json!({"type":"Form.Checkbox","props":{"id":id,"label":label,"value":value.as_bool().unwrap_or(false)}})
}

fn choices(raw: &[(&str, &str)]) -> Vec<(String, String)> {
    raw.iter()
        .map(|(value, title)| (value.to_string(), title.to_string()))
        .collect()
}

fn property(prop: &Property, values: &Value) -> Value {
    let id = prop.field();
    let value = &values[&id];
    match prop.kind.as_str() {
        "checkbox" => checkbox(&id, &prop.title, value),
        "select" | "status" => {
            let mut options = vec![(String::new(), "None".into())];
            options.extend(prop.options.clone());
            choice_field("Dropdown", &id, &prop.title, value, options)
        }
        "multi_select" => choice_field("TagPicker", &id, &prop.title, value, prop.options.clone()),
        "rich_text" => field("TextArea", &id, &prop.title, value),
        kind => {
            let mut field = field("TextField", &id, &prop.title, value);
            let placeholder = match kind {
                "date" => "YYYY-MM-DD, RFC 3339, or start / end",
                "people" => "Comma-separated Notion user IDs",
                "relation" => "Comma-separated Notion page IDs",
                _ => "",
            };
            if !placeholder.is_empty() {
                field["props"]["placeholder"] = json!(placeholder);
            }
            field
        }
    }
}

fn regular_row(
    actions: &mut Actions,
    item: &Item,
    prefs: &Value,
    markers: &Markers,
) -> Vec<String> {
    let app = Action::Open(item.clone(), false);
    let navigate = Action::Navigate(item.clone());
    let mut ids = Vec::new();
    let title = if item.is_page() {
        "Preview Page"
    } else {
        "Browse Data Source"
    };
    if prefs["primaryAction"] == "raycast" {
        ids.push(actions.add(title, navigate));
        ids.push(actions.add("Open in Preferred App", app));
    } else {
        ids.push(actions.add("Open in Preferred App", app));
        ids.push(actions.add(title, navigate));
    }
    ids.push(actions.add("Open in Browser", Action::Open(item.clone(), true)));
    if item.is_page() {
        ids.push(actions.add("Append Content to Page", Action::Append(item.clone())));
        ids.push(actions.add("Edit Properties", Action::Edit(item.clone())));
    } else {
        ids.push(actions.add("Create New Page", Action::Create(item.clone())));
    }
    let pinned = markers.pinned.contains(&item.id);
    ids.push(actions.add(
        if pinned { "Unpin Item" } else { "Pin Item" },
        Action::Pin(item.clone(), !pinned),
    ));
    ids.push(actions.add("Copy URL", Action::Copy(item.clone(), CopyKind::Url)));
    ids.push(actions.add(
        "Copy Formatted URL",
        Action::Copy(item.clone(), CopyKind::Formatted),
    ));
    ids.push(actions.add("Copy Title", Action::Copy(item.clone(), CopyKind::Title)));
    ids.push(actions.add("Paste URL", Action::Copy(item.clone(), CopyKind::Paste)));
    ids.push(actions.add(
        "Create Quick Link to Notion",
        Action::QuickLink(item.clone()),
    ));
    if item.kind != "database" {
        ids.push(actions.add("Move to Trash", Action::Trash(item.clone())));
    }
    ids
}

fn list_screen(
    list: &List,
    actions: &mut Actions,
    prefs: &Value,
    markers: &Markers,
    notice: &str,
) -> Value {
    let mut children = Vec::new();
    if let ListKind::Properties(page, props, values) = &list.kind {
        for prop in props {
            let action = actions.add(
                "Edit Property",
                Action::EditField(page.clone(), prop.clone(), values[&prop.field()].clone()),
            );
            children.push(json!({"type":"List.Item","props":{"id":prop.id,"title":prop.title,"subtitle":prop.kind,"actionIds":[action]}}));
        }
    }
    let mut rows = list.rows.iter().collect::<Vec<_>>();
    if list.query.is_empty() && matches!(list.kind, ListKind::Workspace(Purpose::Search)) {
        rows.sort_by_key(|item| {
            (
                markers
                    .pinned
                    .iter()
                    .position(|id| id == &item.id)
                    .unwrap_or(usize::MAX),
                markers
                    .recent
                    .iter()
                    .position(|id| id == &item.id)
                    .unwrap_or(usize::MAX),
            )
        });
    }
    for item in rows {
        let action_ids = match &list.kind {
            ListKind::Workspace(Purpose::Search) | ListKind::Records(..) => {
                regular_row(actions, item, prefs, markers)
            }
            ListKind::Workspace(purpose) | ListKind::DatabaseSources(purpose, _) => {
                vec![actions.add(
                    match purpose {
                        Purpose::Search => "Browse Data Source",
                        Purpose::Create => "Create Page",
                        Purpose::Capture => "Capture Here",
                        Purpose::Append => "Add Text Here",
                        Purpose::NoteParent => "Use This Parent",
                    },
                    Action::Choose(purpose.clone(), item.clone()),
                )]
            }
            _ => vec![],
        };
        let accessory = if markers.pinned.contains(&item.id) {
            json!([{"text":"Pinned"}])
        } else {
            json!([])
        };
        let kind = match item.kind.as_str() {
            "page" => "Page",
            "database" => "Database",
            _ => "Data source",
        };
        let title = if item.icon.is_empty() {
            item.title.clone()
        } else {
            format!("{} {}", item.icon, item.title)
        };
        children.push(json!({"type":"List.Item","props":{"id":item.id,"title":title,"subtitle":kind,"actionIds":action_ids,"accessories":accessory}}));
    }
    if children.is_empty() {
        children.push(json!({"type":"List.EmptyView","props":{"title":"No accessible items found","description":"Try a different search or connect the integration to the Notion item."}}));
    }
    if !notice.is_empty() {
        children.insert(0,json!({"type":"List.Item","props":{"id":"notion-notice","title":notice,"actionIds":[]}}));
    }
    let mut global = vec![];
    let remote = match &list.kind {
        ListKind::Workspace(purpose) => {
            global.push(actions.add("Find by URL or ID", Action::Lookup(purpose.clone())));
            true
        }
        ListKind::Records(item, _) => {
            global.push(actions.add("Create New Page", Action::Create(item.clone())));
            true
        }
        _ => false,
    };
    if remote {
        global.push(actions.add("Refresh Results", Action::Refresh));
    }
    json!({"type":"List","props":{"searchText":list.query,"filtering":!remote,"__hasServerSearch":remote,
        "searchBarPlaceholder": if remote {"Search Notion"} else {"Filter entries"},"actionIds":global},
        "children":children})
}

fn form_screen(
    form: &Form,
    draft: &Value,
    actions: &mut Actions,
    prefs: &Value,
    notice: &str,
) -> Value {
    let mut children = vec![];
    if !notice.is_empty() {
        children.push(description("Notion", notice));
    }
    let mut ids = vec![];
    match form {
        Form::Lookup(_) => children.push(field(
            "TextField",
            "target",
            "Notion URL or ID",
            &draft["target"],
        )),
        Form::Create(item, props) => {
            children.push(description("Data Source", &item.title));
            children.extend(props.iter().map(|prop| property(prop, draft)));
            children.push(field(
                "TextArea",
                "content",
                "Content (Markdown)",
                &draft["content"],
            ));
            children.push(checkbox(
                "addDateDivider",
                "Insert a date divider",
                &draft["addDateDivider"],
            ));
            let close = prefs["closeAfterCreate"] == true;
            ids.push(actions.add(
                if close {
                    "Create Page and Close"
                } else {
                    "Create Page"
                },
                Action::Submit(close),
            ));
            ids.push(actions.add(
                if close {
                    "Create Page"
                } else {
                    "Create Page and Close"
                },
                Action::Submit(!close),
            ));
        }
        Form::Append(item, _) => {
            children.push(description("Page", &item.title));
            children.push(field(
                "TextArea",
                "textToAppend",
                "Content (Markdown)",
                &draft["textToAppend"],
            ));
            children.push(checkbox("prepend", "Insert at the top", &draft["prepend"]));
            children.push(checkbox(
                "addDateDivider",
                "Insert a date divider",
                &draft["addDateDivider"],
            ));
        }
        Form::Note(parent) => {
            let name = prefs["notes_page_name"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("NOTES");
            let date =
                domain::date_title(prefs["note_date_format"].as_str().unwrap_or("yyyy-MM-dd"));
            children.push(description("Destination", &format!("{name} ? {date}")));
            if let Some(parent) = parent {
                children.push(description("Parent", &parent.title));
            }
            children.push(field("TextArea", "note", "Note", &draft["note"]));
            ids.push(actions.add("Add Note", Action::Submit(false)));
            ids.push(actions.add("Choose Parent Page", Action::NoteParent));
            if parent.is_some() {
                ids.push(actions.add("Clear Parent Page", Action::ClearNoteParent));
            }
        }
        Form::Capture(item) => {
            children.push(description("Destination", &item.title));
            children.push(field("TextField", "url", "URL", &draft["url"]));
            children.push(choice_field(
                "Dropdown",
                "captureAs",
                "Capture As",
                &draft["captureAs"],
                choices(&[
                    ("url", "Bookmark"),
                    ("full", "Full Page"),
                    ("ai", "Summarize Page with AI"),
                ]),
            ));
            children.push(checkbox(
                "addDateDivider",
                "Insert a date divider",
                &draft["addDateDivider"],
            ));
            ids.push(actions.add("Capture", Action::Submit(false)));
            ids.push(actions.add("Use Selected URL", Action::UseSelection));
            ids.push(actions.add("Use Clipboard URL", Action::UseClipboard));
        }
        Form::Edit(item, prop) => {
            children.push(description("Page", &item.title));
            children.push(property(prop, draft));
        }
    }
    if ids.is_empty() {
        let title = match form {
            Form::Lookup(_) => "Find Item",
            Form::Edit(..) => "Save Property",
            _ => form.title(),
        };
        ids.push(actions.add(title, Action::Submit(false)));
    }
    json!({"type":"Form","props":{"actionIds":ids},"children":children})
}

fn detail(text: &str, notice: &str, action_ids: Vec<String>) -> Value {
    let markdown = if notice.is_empty() {
        text.to_owned()
    } else {
        format!("{notice}\n\n{text}")
    };
    json!({"type":"Detail","props":{"markdown":markdown,"actionIds":action_ids}})
}

pub fn render(session: &mut Session, prefs: &Value) -> Value {
    session.revision = session.revision.wrapping_add(1);
    let mut actions = Actions {
        revision: session.revision,
        ..Actions::default()
    };
    let (title, mut root) = match &session.screen {
        Screen::Setup => {
            let ids = vec![
                actions.add("Open Extension Preferences", Action::Settings),
                actions.add("Manage Page Access", Action::Access),
            ];
            ("Notion",detail("Set **Internal Integration Secret** in Notion extension preferences, then reopen this command. Connect that integration to the pages and databases you want to use.\n\nRaycast browser sign-in uses a separate provider. This extension connects through an internal integration.",&session.notice,ids))
        }
        Screen::Connection(status) => {
            let ids = vec![
                actions.add("Test Connection", Action::TestConnection),
                actions.add("Open Extension Preferences", Action::Settings),
                actions.add("Manage Page Access", Action::Access),
            ];
            (
                "Manage Notion Connection",
                detail(status, &session.notice, ids),
            )
        }
        Screen::List(list) => (
            list.title(),
            list_screen(list, &mut actions, prefs, &session.markers, &session.notice),
        ),
        Screen::Form(form, draft) => (
            form.title(),
            form_screen(form, draft, &mut actions, prefs, &session.notice),
        ),
        Screen::Preview(item, markdown, metadata, show) => {
            let ids = vec![
                actions.add("Open in Preferred App", Action::Open(item.clone(), false)),
                actions.add("Append Content to Page", Action::Append(item.clone())),
                actions.add("Edit Properties", Action::Edit(item.clone())),
                actions.add(
                    if *show {
                        "Hide Properties"
                    } else {
                        "Show Properties"
                    },
                    Action::ToggleMetadata,
                ),
                actions.add("Copy URL", Action::Copy(item.clone(), CopyKind::Url)),
                actions.add("Paste URL", Action::Copy(item.clone(), CopyKind::Paste)),
            ];
            let mut root = detail(markdown, &session.notice, ids);
            if *show {
                root["metadata"] = json!({"type":"Detail.Metadata","children":metadata.iter().map(
                    |(title,text)|json!({"type":"Detail.Metadata.Label","props":{"title":title,"text":text}})).collect::<Vec<_>>()});
            }
            (item.title.as_str(), root)
        }
        Screen::Trashed(item) => {
            let ids = vec![actions.add("Restore from Trash", Action::Restore(item.clone()))];
            (
                item.title.as_str(),
                detail("Moved to the Notion Trash.", &session.notice, ids),
            )
        }
        Screen::Saved(item, message) => {
            let ids = vec![
                actions.add("Open Page", Action::Open(item.clone(), false)),
                actions.add("Copy Page URL", Action::Copy(item.clone(), CopyKind::Url)),
            ];
            (item.title.as_str(), detail(message, &session.notice, ids))
        }
    };
    if !session.history.is_empty() {
        let back = actions.add("Back", Action::Back);
        root["props"]["backActionId"] = json!(back);
        if let Some(ids) = root["props"]["actionIds"].as_array_mut() {
            ids.push(json!(back));
        }
    }
    let root_type = string(&root, "type");
    let mut value = view(title, root.clone(), actions.descriptors);
    value["refreshMs"] = Value::Null;
    if root_type == "List" {
        value["hasMore"] =
            json!(matches!(&session.screen,Screen::List(list) if list.cursor.is_some()));
    }
    session.actions = actions.handlers;
    value
}
