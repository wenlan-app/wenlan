// SPDX-License-Identifier: Apache-2.0
//! Bounded, credential-free public text retrieval. Never writes to the database.
use dom_query::Document;
use futures::future::BoxFuture;
use reqwest::{header, redirect, Client, Response, Url};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Host;
use wenlan_types::responses::{FetchWebpageResponse, WebpageExtraction};

pub(crate) const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 2048;
const MAX_AUTHOR_CHARS: usize = 200;
const MAX_URL_BYTES: usize = 4096;
const MAX_REDIRECTS: usize = 3;
pub(crate) const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchError {
    InvalidUrl,
    BlockedDestination,
    DnsFailed,
    RedirectLimit,
    UnsupportedContentType,
    UnsupportedEncoding,
    TooLarge,
    Timeout,
    Transport,
    HttpStatus(u16),
    NoReadableText,
}

pub(crate) fn validate_url(input: &str) -> Result<Url, FetchError> {
    if input.len() > MAX_URL_BYTES || input.chars().any(char::is_control) {
        return Err(FetchError::InvalidUrl);
    }
    let mut url = Url::parse(input).map_err(|_| FetchError::InvalidUrl)?;
    let port = match url.scheme() {
        "http" => 80,
        "https" => 443,
        _ => return Err(FetchError::InvalidUrl),
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(port)
    {
        return Err(FetchError::InvalidUrl);
    }
    let host = url.host().ok_or(FetchError::InvalidUrl)?;
    match host {
        Host::Ipv4(ip) if !public_ip(ip.into()) => return Err(FetchError::BlockedDestination),
        Host::Ipv6(ip) if !public_ip(ip.into()) => return Err(FetchError::BlockedDestination),
        Host::Domain(domain) => {
            let host = domain.trim_end_matches('.');
            if !host.contains('.')
                || [".localhost", ".local", ".internal", ".home.arpa"]
                    .iter()
                    .any(|suffix| host.ends_with(suffix))
            {
                return Err(FetchError::BlockedDestination);
            }
        }
        _ => {}
    }
    url.set_fragment(None);
    Ok(url)
}

/// Conservative global-unicast policy based on IANA special-purpose registries.
/// Reject mapped/translated IPv6 and transition ranges, even public exceptions.
pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let value = u32::from(ip);
            ![
                (0x00000000, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 3),
            ]
            .iter()
            .any(|(network, prefix)| value >> (32 - prefix) == network >> (32 - prefix))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            s[0] & 0xe000 == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] & 0xf000 == 0)
        }
    }
}

pub(crate) fn validate_addresses(addresses: &[SocketAddr], port: u16) -> Result<(), FetchError> {
    if addresses.is_empty() || addresses.len() > 16 {
        return Err(FetchError::DnsFailed);
    }
    if addresses
        .iter()
        .any(|address| !public_ip(address.ip()) || address.port() != port)
    {
        return Err(FetchError::BlockedDestination);
    }
    Ok(())
}

// The seam is internal: production has no env/HTTP escape hatch for private IPs.
pub(crate) trait Transport: Send + Sync {
    fn resolve<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<Vec<SocketAddr>, FetchError>>;
    fn get<'a>(
        &'a self,
        url: &'a Url,
        pinned: &'a [SocketAddr],
    ) -> BoxFuture<'a, Result<Response, FetchError>>;
}

pub(crate) struct PublicTransport;
pub(crate) fn pinned_client(url: &Url, pinned: &[SocketAddr]) -> Result<Client, FetchError> {
    Client::builder()
        .no_proxy()
        .redirect(redirect::Policy::none())
        .referer(false)
        .http1_only()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(Duration::from_secs(5))
        .timeout(TOTAL_TIMEOUT)
        // Retain the URL hostname for Host and TLS certificate/SNI validation.
        // A fresh client per hop prevents a connection pool from outliving pinning.
        .resolve_to_addrs(url.host_str().ok_or(FetchError::InvalidUrl)?, pinned)
        .user_agent("Wenlan-PublicText/1.0")
        .build()
        .map_err(|_| FetchError::Transport)
}

impl Transport for PublicTransport {
    fn resolve<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<Vec<SocketAddr>, FetchError>> {
        Box::pin(async move {
            let port = url.port_or_known_default().ok_or(FetchError::InvalidUrl)?;
            if let Some(host) = url.host() {
                match host {
                    Host::Ipv4(ip) => return Ok(vec![SocketAddr::new(ip.into(), port)]),
                    Host::Ipv6(ip) => return Ok(vec![SocketAddr::new(ip.into(), port)]),
                    _ => {}
                }
            }
            // Native DNS can outlive a cancelled async request. Keep its permit
            // inside the blocking job so timeouts cannot create unbounded jobs.
            static DNS_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
            let permit = DNS_SLOTS
                .get_or_init(|| Arc::new(Semaphore::new(4)))
                .clone()
                .try_acquire_owned()
                .map_err(|_| FetchError::DnsFailed)?;
            let host = url.host_str().ok_or(FetchError::InvalidUrl)?.to_owned();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                (host.as_str(), port)
                    .to_socket_addrs()
                    .map(|addresses| addresses.take(17).collect())
                    .map_err(|_| FetchError::DnsFailed)
            })
            .await
            .map_err(|_| FetchError::DnsFailed)?
        })
    }
    fn get<'a>(
        &'a self,
        url: &'a Url,
        pinned: &'a [SocketAddr],
    ) -> BoxFuture<'a, Result<Response, FetchError>> {
        Box::pin(async move {
            pinned_client(url, pinned)?
                .get(url.clone())
                .header(header::ACCEPT, "text/html, text/plain")
                .header(header::ACCEPT_ENCODING, "identity")
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        FetchError::Timeout
                    } else {
                        FetchError::Transport
                    }
                })
        })
    }
}

pub(crate) async fn fetch_with<T: Transport>(
    transport: &T,
    original: &str,
    permit: Option<OwnedSemaphorePermit>,
) -> Result<FetchWebpageResponse, FetchError> {
    let mut url = validate_url(original)?;
    for hop in 0..=MAX_REDIRECTS {
        let addresses = transport.resolve(&url).await?;
        validate_addresses(
            &addresses,
            url.port_or_known_default().ok_or(FetchError::InvalidUrl)?,
        )?;
        let mut response = transport.get(&url, &addresses).await?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            if hop == MAX_REDIRECTS {
                return Err(FetchError::RedirectLimit);
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|header| header.to_str().ok())
                .ok_or(FetchError::InvalidUrl)?;
            let joined = url.join(location).map_err(|_| FetchError::InvalidUrl)?;
            url = validate_url(joined.as_str())?;
            continue;
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(FetchError::HttpStatus(response.status().as_u16()));
        }
        // No automatic decompression, including when feature-unified dependencies
        // enable it. An ignored identity request is an explicit fallback error.
        for encoding in response.headers().get_all(header::CONTENT_ENCODING) {
            if !encoding
                .to_str()
                .is_ok_and(|value| value.trim().eq_ignore_ascii_case("identity"))
            {
                return Err(FetchError::UnsupportedEncoding);
            }
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .ok_or(FetchError::UnsupportedContentType)?
            .to_owned();
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !matches!(mime.as_str(), "text/html" | "text/plain") {
            return Err(FetchError::UnsupportedContentType);
        }
        // First version supports UTF-8 only; never silently corrupt another charset.
        if content_type.split(';').skip(1).any(|part| {
            part.trim().split_once('=').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("charset")
                    && !matches!(
                        value.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                        "utf-8" | "utf8" | "us-ascii"
                    )
            })
        }) {
            return Err(FetchError::UnsupportedContentType);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(FetchError::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                FetchError::Timeout
            } else {
                FetchError::Transport
            }
        })? {
            if chunk.len() > MAX_BYTES - body.len() {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(body).map_err(|_| FetchError::UnsupportedContentType)?;
        let final_url = url.to_string();
        let original = original.to_owned();
        return tokio::task::spawn_blocking(move || {
            // If the HTTP deadline expires while parsing, the permit stays held
            // until this bounded job exits; canceled requests cannot fan out parsers.
            let _permit = permit;
            extract(&text, &mime, &original, &final_url)
        })
        .await
        .map_err(|_| FetchError::Transport)?;
    }
    Err(FetchError::RedirectLimit)
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect::<String>()
        .trim()
        .to_owned()
}

pub(crate) fn extract(
    text: &str,
    mime: &str,
    original: &str,
    final_url: &str,
) -> Result<FetchWebpageResponse, FetchError> {
    let (title, content, extraction, description, author) = if mime == "text/plain" {
        (
            text.lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(final_url)
                .to_owned(),
            clean(text),
            WebpageExtraction::PlainText,
            None,
            None,
        )
    } else {
        let doc = Document::from(text);
        if doc.root().descendants_it().take(100_001).count() > 100_000 {
            return Err(FetchError::TooLarge);
        }
        let title = doc.select_single("title").text().to_string();
        let meta = |selector: &str| {
            doc.select(selector)
                .nodes()
                .iter()
                .filter_map(|node| node.attr("content"))
                .map(|content| clean(&content))
                .find(|content| !content.is_empty())
        };
        // What link previews show: social sites serve it to bots even when the
        // body is only a sign-in shell.
        let preview_title = meta("meta[property='og:title'],meta[name='twitter:title']");
        let preview = meta(
            "meta[property='og:description'],meta[name='twitter:description'],meta[name='description']",
        );
        // Who made it: an article's byline, or a video's channel.
        let author = meta("meta[name='author'],[itemprop='author'] [itemprop='name']");
        doc.select("script,style,noscript,template,nav,footer,aside,iframe,object,embed,svg,canvas,form,button,input,select,textarea,dialog,[hidden],[aria-hidden='true'],body > header").remove();
        // Ignore explicit inline hiding; no browser/JS/CSS execution or paywall bypass.
        for node in doc.select("[style]").nodes() {
            let style = node
                .attr("style")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .replace(' ', "");
            if style.contains("display:none") || style.contains("visibility:hidden") {
                node.remove_from_parent();
            }
        }
        let article = doc.select_single("article").formatted_text().to_string();
        let main = doc
            .select_single("main,[role='main']")
            .formatted_text()
            .to_string();
        let body = doc.select_single("body").formatted_text().to_string();
        let (content, method) = if meaningful(&article) {
            (article, WebpageExtraction::HtmlSemantic)
        } else if meaningful(&main) {
            (main, WebpageExtraction::HtmlSemantic)
        } else if meaningful(&body) {
            (body, WebpageExtraction::HtmlBody)
        } else if let Some(text) = preview.as_ref().filter(|text| alphanumerics(text) >= 8) {
            // A short post is still worth keeping; fewer than 8 letters is a label.
            (text.clone(), WebpageExtraction::HtmlMetadata)
        } else {
            (body, WebpageExtraction::HtmlBody)
        };
        // A bare site name such as "Threads" or "Instagram" names no page.
        let title = match preview_title {
            Some(preview_title)
                if !title.trim().contains(char::is_whitespace)
                    && preview_title.chars().count() > title.trim().chars().count() =>
            {
                preview_title
            }
            _ if title.trim().is_empty() => doc.select_single("h1").text().to_string(),
            _ => title,
        };
        (title, clean(&content), method, preview, author)
    };
    if content.len() > MAX_BYTES {
        return Err(FetchError::TooLarge);
    }
    if !meaningful(&content) && extraction != WebpageExtraction::HtmlMetadata {
        return Err(FetchError::NoReadableText);
    }
    let title = clean(&title).chars().take(512).collect::<String>();
    Ok(FetchWebpageResponse {
        url: original.to_owned(),
        final_url: final_url.to_owned(),
        title: if title.is_empty() {
            final_url.to_owned()
        } else {
            title
        },
        content,
        content_type: mime.to_owned(),
        extraction,
        description: description.map(|text| text.chars().take(MAX_DESCRIPTION_CHARS).collect()),
        author: author.map(|text| text.chars().take(MAX_AUTHOR_CHARS).collect()),
        untrusted: true,
    })
}

fn alphanumerics(text: &str) -> usize {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .take(40)
        .count()
}

fn meaningful(text: &str) -> bool {
    alphanumerics(text) >= 40
}

#[cfg(test)]
#[path = "web_fetch_tests.rs"]
mod tests;
