use reqwest::{header, Client, Method, Response, Url};
use serde_json::{json, Value};
use std::{fmt, time::Duration};

pub const VERSION: &str = "2026-03-11";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_WRITE_BYTES: usize = 450 * 1024;

pub fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_default()
}

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Network,
    InvalidReply,
    Status(u16, Option<u64>),
}

impl Error {
    pub fn status(&self, code: u16) -> bool {
        matches!(self, Self::Status(status, _) if *status == code)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Network => f.write_str("Notion did not return a response. Check the connection. If you were saving, check Notion before trying again."),
            Self::InvalidReply => f.write_str("Notion returned an unreadable or oversized response. If you were saving, check Notion before trying again."),
            Self::Status(400, _) => f.write_str("Notion rejected the request. Check property values and the destination."),
            Self::Status(401, _) => f.write_str("Notion rejected the integration secret. Update it in extension preferences."),
            Self::Status(403 | 404, _) => f.write_str("The integration cannot access this Notion item. Connect the integration to the item and check its capabilities."),
            Self::Status(409, _) => f.write_str("Notion reported a conflict. Check the current content and try again."),
            Self::Status(429, Some(seconds)) => write!(f,"Notion is rate limiting this integration. Try again in {seconds} seconds."),
            Self::Status(429, None) => f.write_str("Notion is rate limiting this integration. Try again shortly."),
            Self::Status(500..=599, _) => f.write_str("Notion reports a service problem. If you were saving, check Notion before trying again."),
            Self::Status(status, _) => write!(f, "Notion returned HTTP {status}."),
        }
    }
}
impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}

pub fn id(raw: &str) -> Option<String> {
    let compact = raw.replace('-', "");
    if (raw.len() != 32 && raw.len() != 36)
        || (raw.len() == 36 && [8, 13, 18, 23].iter().any(|at| raw.as_bytes()[*at] != b'-'))
        || compact.len() != 32
        || !compact.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let compact = compact.to_ascii_lowercase();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &compact[..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..]
    ))
}

fn checked_id(raw: &str) -> Result<String, Error> {
    id(raw).ok_or_else(|| Error::Invalid("This Notion item has an invalid ID.".into()))
}

pub struct Transport {
    http: Client,
    origin: Url,
}

impl Transport {
    pub fn new() -> Result<Self, String> {
        #[cfg(not(feature = "fixture"))]
        let origin = Url::parse("https://api.notion.com/v1/").expect("fixed Notion URL");
        #[cfg(feature = "fixture")]
        let origin = {
            let raw = std::env::var("TEZBAR_NOTION_FIXTURE_ORIGIN")
                .map_err(|_| "Fixture builds require a loopback origin.")?;
            let base = Url::parse(&raw).map_err(|_| "Invalid fixture origin.")?;
            if base.scheme() != "http"
                || !matches!(base.host_str(), Some("127.0.0.1" | "[::1]"))
                || !base.username().is_empty()
                || base.password().is_some()
                || base.port().is_none()
                || base.path() != "/"
                || base.query().is_some()
                || base.fragment().is_some()
            {
                return Err(
                    "Fixture origin must be HTTP on a literal loopback address and explicit port."
                        .into(),
                );
            }
            base.join("v1/").map_err(|_| "Invalid fixture origin.")?
        };

        let mut builder = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(4))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never());
        if cfg!(feature = "fixture") {
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|_| "Cannot initialize Notion HTTPS.")?;
        Ok(Self { http, origin })
    }

    pub fn authorize<'a>(&'a self, secret: &str) -> Result<Api<'a>, Error> {
        let secret = secret.trim();
        if secret.is_empty() {
            return Err(Error::Invalid(
                "Set the Internal Integration Secret in extension preferences.".into(),
            ));
        }
        #[cfg(feature = "fixture")]
        if secret != "ntn_fixture_only" {
            return Err(Error::Invalid(
                "Fixture builds accept only the synthetic integration secret.".into(),
            ));
        }
        let mut token =
            header::HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|_| {
                Error::Invalid("The integration secret contains invalid characters.".into())
            })?;
        token.set_sensitive(true);
        Ok(Api {
            transport: self,
            token,
        })
    }

    #[cfg(feature = "fixture")]
    pub fn origin(&self) -> &Url {
        &self.origin
    }
}

pub struct Api<'a> {
    transport: &'a Transport,
    token: header::HeaderValue,
}

#[derive(Debug)]
pub struct Batch {
    pub rows: Vec<Value>,
    pub cursor: Option<String>,
}

impl Batch {
    fn parse(mut reply: Value, previous: Option<&str>) -> Result<Self, Error> {
        let cursor = if reply["has_more"] == true {
            let value = string(&reply, "next_cursor");
            if value.is_empty() || value.len() > 4096 || Some(value) == previous {
                return Err(Error::InvalidReply);
            }
            Some(value.to_owned())
        } else if reply["has_more"] == false {
            None
        } else {
            return Err(Error::InvalidReply);
        };
        let Value::Array(rows) = reply["results"].take() else {
            return Err(Error::InvalidReply);
        };
        if rows.len() > 100 {
            return Err(Error::InvalidReply);
        }
        Ok(Self { rows, cursor })
    }
}

pub async fn read_bounded(mut response: Response, limit: usize) -> Result<Vec<u8>, Error> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(Error::InvalidReply);
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
        if data.len() + chunk.len() > limit {
            return Err(Error::InvalidReply);
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

impl Api<'_> {
    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value, Error> {
        let url = self
            .transport
            .origin
            .join(path)
            .map_err(|_| Error::InvalidReply)?;
        let mut request = self
            .transport
            .http
            .request(method, url)
            .header(header::AUTHORIZATION, self.token.clone())
            .header("Notion-Version", VERSION)
            .query(query);
        if let Some(body) = body {
            let bytes = serde_json::to_vec(&body).map_err(|_| Error::InvalidReply)?;
            if bytes.len() > MAX_WRITE_BYTES {
                return Err(Error::Invalid(
                    "The Notion request is too large. Shorten the content and try again.".into(),
                ));
            }
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(bytes);
        }
        let response = request.send().await.map_err(|_| Error::Network)?;
        if !response.status().is_success() {
            return Err(Error::Status(
                response.status().as_u16(),
                response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse().ok()),
            ));
        }
        serde_json::from_slice(&read_bounded(response, MAX_RESPONSE_BYTES).await?)
            .map_err(|_| Error::InvalidReply)
    }

    pub async fn me(&self) -> Result<Value, Error> {
        self.call(Method::GET, "users/me", &[], None).await
    }

    pub async fn search(
        &self,
        query: &str,
        filter: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Batch, Error> {
        let mut body = json!({"page_size":50,"sort":{"direction":"descending","timestamp":"last_edited_time"}});
        if !query.is_empty() {
            body["query"] = json!(query);
        }
        if let Some(kind) = filter {
            body["filter"] = json!({"property":"object","value":kind});
        }
        Self::cursor(&mut body, cursor);
        Batch::parse(
            self.call(Method::POST, "search", &[], Some(body)).await?,
            cursor,
        )
    }

    fn cursor(body: &mut Value, cursor: Option<&str>) {
        if let Some(cursor) = cursor {
            body["start_cursor"] = json!(cursor);
        }
    }

    pub async fn query(
        &self,
        source: &str,
        title_property: &str,
        query: &str,
        cursor: Option<&str>,
    ) -> Result<Batch, Error> {
        let mut body = json!({"page_size":50,"sorts":[{"timestamp":"last_edited_time","direction":"descending"}]});
        if !query.is_empty() {
            body["filter"] = json!({"property":title_property,"title":{"contains":query}});
        }
        Self::cursor(&mut body, cursor);
        Batch::parse(
            self.call(
                Method::POST,
                &format!("data_sources/{}/query", checked_id(source)?),
                &[],
                Some(body),
            )
            .await?,
            cursor,
        )
    }

    pub async fn page(&self, page: &str) -> Result<Value, Error> {
        self.call(
            Method::GET,
            &format!("pages/{}", checked_id(page)?),
            &[],
            None,
        )
        .await
    }

    pub async fn source(&self, source: &str) -> Result<Value, Error> {
        self.call(
            Method::GET,
            &format!("data_sources/{}", checked_id(source)?),
            &[],
            None,
        )
        .await
    }

    pub async fn database(&self, database: &str) -> Result<Value, Error> {
        self.call(
            Method::GET,
            &format!("databases/{}", checked_id(database)?),
            &[],
            None,
        )
        .await
    }

    pub async fn children(&self, block: &str, cursor: Option<&str>) -> Result<Batch, Error> {
        let mut args = vec![("page_size", "50")];
        if let Some(cursor) = cursor {
            args.push(("start_cursor", cursor));
        }
        Batch::parse(
            self.call(
                Method::GET,
                &format!("blocks/{}/children", checked_id(block)?),
                &args,
                None,
            )
            .await?,
            cursor,
        )
    }

    pub async fn append(
        &self,
        page: &str,
        children: Vec<Value>,
        first: bool,
    ) -> Result<Value, Error> {
        let mut body = json!({"children":children});
        if first {
            body["position"] = json!({"type":"start"});
        }
        self.call(
            Method::PATCH,
            &format!("blocks/{}/children", checked_id(page)?),
            &[],
            Some(body),
        )
        .await
    }

    pub async fn create(
        &self,
        parent: &str,
        source: bool,
        properties: Value,
        children: Vec<Value>,
    ) -> Result<Value, Error> {
        let parent = checked_id(parent)?;
        let parent = if source {
            json!({"type":"data_source_id","data_source_id":parent})
        } else {
            json!({"type":"page_id","page_id":parent})
        };
        let mut body = json!({"parent":parent,"properties":properties});
        if !children.is_empty() {
            body["children"] = json!(children);
        }
        self.call(Method::POST, "pages", &[], Some(body)).await
    }

    pub async fn update_properties(&self, page: &str, properties: Value) -> Result<Value, Error> {
        self.call(
            Method::PATCH,
            &format!("pages/{}", checked_id(page)?),
            &[],
            Some(json!({"properties":properties})),
        )
        .await
    }

    pub async fn trash(&self, item: &str, kind: &str, trashed: bool) -> Result<Value, Error> {
        let base = match kind {
            "page" => "pages",
            "data_source" => "data_sources",
            _ => return Err(Error::Invalid("This item cannot be moved to Trash.".into())),
        };
        self.call(
            Method::PATCH,
            &format!("{base}/{}", checked_id(item)?),
            &[],
            Some(json!({"in_trash":trashed})),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_ids_and_cursors() {
        assert_eq!(
            id("A0000000000000000000000000000001").unwrap(),
            "a0000000-0000-0000-0000-000000000001"
        );
        assert!(id("a0000000-0000-0000-0000/000000000001").is_none());
        assert!(Batch::parse(
            json!({"results":[],"has_more":true,"next_cursor":"same"}),
            Some("same")
        )
        .is_err());
        assert!(Batch::parse(json!({"results":[],"has_more":true}), None).is_err());
        assert_eq!(
            Batch::parse(json!({"results":[],"has_more":false}), None)
                .unwrap()
                .cursor,
            None
        );
    }
}
