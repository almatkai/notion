use crate::{api, blocks};
use reqwest::{header, Client, Url};
use scraper::{Html, Selector};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

const HTML_LIMIT: usize = 2 * 1024 * 1024;
const ARTICLE_LIMIT: usize = 24_000;

#[derive(Debug)]
pub struct Article {
    pub title: String,
    pub paragraphs: Vec<String>,
}

pub fn target(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    let with_scheme;
    let candidate = if trimmed.starts_with("www.") {
        with_scheme = format!("https://{trimmed}");
        &with_scheme
    } else {
        trimmed
    };
    blocks::url(candidate)
        .filter(|url| {
            matches!(url.port_or_known_default(), Some(80 | 443)) || cfg!(feature = "fixture")
        })
        .ok_or_else(|| "Enter a web URL without embedded credentials.".into())
}

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !matches!(
                (a, b, c),
                (0 | 10 | 127 | 224..=255, _, _)
                    | (100, 64..=127, _)
                    | (169, 254, _)
                    | (172, 16..=31, _)
                    | (192, 0, 0 | 2)
                    | (192, 88, 99)
                    | (192, 168, _)
                    | (198, 18 | 19, _)
                    | (198, 51, 100)
                    | (203, 0, 113)
            )
        }
        IpAddr::V6(ip) => {
            let [a, b, _, _, _, _, _, _] = ip.segments();
            (0x2000..0x4000).contains(&a)
                && !(a == 0x2001 && (b <= 0x1ff || b == 0xdb8))
                && a != 0x2002
                && a != 0x3fff
        }
    }
}

async fn client_for(url: &Url, transport: &api::Transport) -> Result<Client, String> {
    let host = url.host_str().ok_or("Missing capture host.")?;
    let port = url.port_or_known_default().ok_or("Missing capture port.")?;
    #[cfg(feature = "fixture")]
    let fixture = url.scheme() == transport.origin().scheme()
        && url.host_str() == transport.origin().host_str()
        && url.port() == transport.origin().port();
    #[cfg(not(feature = "fixture"))]
    let fixture = {
        let _ = transport;
        false
    };
    if cfg!(feature = "fixture") && !fixture {
        return Err("Fixture builds capture only from the configured loopback origin.".into());
    }

    let bare_host = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = match bare_host.parse::<IpAddr>() {
        Ok(addr) => vec![SocketAddr::new(addr, port)],
        Err(_) => tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| "Could not resolve the page address.")?
            .take(33)
            .collect(),
    };
    if addrs.is_empty()
        || addrs.len() > 32
        || (!fixture && addrs.iter().any(|addr| !public(addr.ip())))
    {
        return Err("Quick Capture can read articles only from public web addresses.".into());
    }
    Client::builder()
        .user_agent("Tezbar Notion Quick Capture")
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(6))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .resolve_to_addrs(host, &addrs)
        .build()
        .map_err(|_| "Cannot initialize Quick Capture.".into())
}

pub async fn article(transport: &api::Transport, url: &Url) -> Result<Article, String> {
    tokio::time::timeout(Duration::from_secs(14), async {
        let mut url = url.clone();
        for _ in 0..4 {
            let response = client_for(&url, transport)
                .await?
                .get(url.clone())
                .send()
                .await
                .map_err(|_| "Could not download the page.")?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or("The page returned an invalid redirect.")?;
                let redirected = url
                    .join(location)
                    .map_err(|_| "The page returned an invalid redirect.")?;
                url = target(redirected.as_str())?;
                continue;
            }
            if !response.status().is_success() {
                return Err(format!(
                    "The page returned HTTP {}.",
                    response.status().as_u16()
                ));
            }
            if response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|kind| {
                    !kind.starts_with("text/html") && !kind.starts_with("application/xhtml+xml")
                })
            {
                return Err("The address did not return an HTML page.".into());
            }
            let data = api::read_bounded(response, HTML_LIMIT)
                .await
                .map_err(|_| "The page could not be read within the download limit.")?;
            return extract(&String::from_utf8_lossy(&data), url.as_str());
        }
        Err("The page redirected too many times.".into())
    })
    .await
    .map_err(|_| "The page took too long to download.")?
}

fn extract(source: &str, fallback: &str) -> Result<Article, String> {
    fn selector(raw: &str) -> Selector {
        Selector::parse(raw).expect("fixed reader selector")
    }
    fn normalize<'a>(text: impl Iterator<Item = &'a str>) -> String {
        text.collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    let html = Html::parse_document(source);
    let title = html
        .select(&selector("title"))
        .next()
        .map(|node| normalize(node.text()))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
        .chars()
        .take(300)
        .collect();
    let mut paragraphs = Vec::new();
    let mut remaining = ARTICLE_LIMIT;
    let full_body =
        selector("body p, body h1, body h2, body h3, body pre, body li, body blockquote");
    let article_body = selector("article p, main p, article h1, main h1, article h2, main h2, article h3, main h3, article pre, main pre, article li, main li, article blockquote, main blockquote");
    let elements = if html.select(&article_body).next().is_some() {
        &article_body
    } else {
        &full_body
    };
    for node in html.select(elements) {
        if node
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .any(|parent| {
                matches!(
                    parent.value().name(),
                    "nav" | "aside" | "footer" | "header" | "form" | "script" | "style"
                ) || parent.attr("hidden").is_some()
                    || parent.attr("aria-hidden") == Some("true")
            })
        {
            continue;
        }
        let paragraph = normalize(node.text());
        if !paragraph.is_empty() && paragraphs.last() != Some(&paragraph) {
            let paragraph: String = paragraph.chars().take(remaining).collect();
            remaining -= paragraph.chars().count();
            paragraphs.push(paragraph);
            if remaining == 0 || paragraphs.len() >= 90 {
                return Err("This article exceeds the capture limit. Capture it as a bookmark or use Add Text to Page with an excerpt.".into());
            }
        }
    }
    if paragraphs.is_empty() {
        return Err(
            "No readable article text was found. You can still capture the address as a bookmark."
                .into(),
        );
    }
    Ok(Article { title, paragraphs })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reader_prefers_article_and_removes_navigation() {
        let article=extract("<title>Test &amp; Read</title><nav><p>Skip</p></nav><main><h1>Start</h1><p>Article <em>body</em>.</p></main><footer><p>Skip</p></footer>","test").unwrap();
        assert_eq!(article.title, "Test & Read");
        assert_eq!(article.paragraphs, ["Start", "Article body."]);
    }
    #[test]
    fn capture_rejects_non_public_addresses_and_embedded_credentials() {
        for raw in [
            "127.0.0.1",
            "100.64.1.2",
            "169.254.169.254",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2002:7f00:0001::1",
        ] {
            assert!(!public(raw.parse().unwrap()), "{raw}");
        }
        assert!(public("8.8.8.8".parse().unwrap()));
        assert!(public("2606:4700::1".parse().unwrap()));
        assert!(target("http://user:secret@example.com").is_err());
        assert!(target("file:///etc/hosts").is_err());
    }
}
