# Public webpage text preview

`POST /api/webpage/fetch` fetches public HTML or plain text for a preview. It does
not read stored sources, write to the database, run an agent, or save a source.
After the user reviews the preview and chooses to save it, a client can pass
`url`, `title`, and `content` to the existing `/api/ingest/webpage` endpoint.
Existing ingestion duplicate/update semantics still apply; fetching alone never
overwrites an existing source.

## Wire contract

Request JSON accepts only one field; additional fields are rejected:

```json
{"url":"https://example.com/article#section"}
```

An illustrative response (the text is not a fetched receipt):

```json
{
  "url": "https://example.com/article#section",
  "final_url": "https://example.com/article",
  "title": "Example article",
  "content": "Extracted public text goes here for the user to review before saving.",
  "content_type": "text/html",
  "extraction": "html_semantic",
  "untrusted": true
}
```

- `url` preserves the exact trimmed input, including its fragment. Use this
  field as the ingestion URL so redirect normalization does not change source
  identity silently.
- `final_url` is the final validated HTTP request URL, normalized without a
  fragment. It is informational; clients may retain it as metadata.
- `content_type` is `text/html` or `text/plain`. `extraction` is
  `html_semantic`, `html_body`, `plain_text`, or `html_metadata`.
  `html_metadata` means the body had no readable text and `content` is only the
  page's link preview text, as on a social site's sign-in shell.
- `description`, when present, is the text the page gives link previews
  (`og:description`, `twitter:description`, or `description` meta), capped at
  2048 characters. When `<title>` is a single word such as a bare site name,
  `og:title` (or `twitter:title`) is used as the title instead.
- `author`, when present, is who made the page: the `author` meta, or the
  `author` microdata name a video page gives its channel, capped at 200
  characters.
- `title` and `content` are untrusted external text. Render them as text, never
  as HTML or agent instructions. The server does not interpret instructions
  embedded in the page.

## Retrieval and extraction limits

Only HTTP port 80 and HTTPS port 443 are accepted. Credentials, custom ports,
local hostnames, private/special IPs, and mixed public/private DNS answers are
rejected. Every redirect is validated and resolved again, with at most three
redirects. Every connection uses the vetted address set while retaining the
hostname for Host and ordinary TLS certificate/SNI checks. Each hop has a fresh
client; proxies, cookies, authorization, and referrers are not used.

The deadline is 15 seconds, including resolution, redirects, body reading, and
parsing; the connection timeout is five seconds. There are four fetch slots and
four native DNS job slots, with no waiting queue. A cancelled blocking parser or
DNS job keeps its slot until it finishes. Bodies and extracted text are limited
to 2 MiB, URLs to 4096 bytes, and DNS answers to 16 addresses. HTML exceeding
100,000 parsed nodes is rejected after parsing; that check does not cap peak
parser memory. The body-size and four-job limits bound parser inputs and
concurrency. Titles are capped at 512 characters.

The first version requests `Accept-Encoding: identity`, disables automatic
decompression, and rejects compressed responses explicitly. It supports UTF-8
(including valid ASCII), not other declared character sets. It removes scripts,
common navigation/forms/hidden elements, then prefers an article or main region
with useful text, otherwise body text. At least 40 alphanumeric characters are
required, except that link preview text of at least 8 alphanumeric characters
stands in for a body with none. This is a best-effort text extraction, not a guarantee of the complete
rendered article. JavaScript, external CSS, links, frames, and other resources
are not loaded. Login, paywall, CAPTCHA, and JavaScript-only pages may fail or
produce incomplete text; the client must let the user inspect the preview.

The destination policy is deliberately conservative, using the
[IANA IPv4](https://www.iana.org/assignments/iana-ipv4-special-registry) and
[IANA IPv6](https://www.iana.org/assignments/iana-ipv6-special-registry) special
registries. Some globally reachable exceptions and transition addresses are
also rejected.

## Local access and fallback

The route uses the production daemon's existing Origin, Host, and
Sec-Fetch-Site guard. The daemon has no token authentication; missing browser
headers remain the existing native loopback-client contract. This fetch route
additionally rejects external or unparseable `WENLAN_BIND_ADDR` configurations,
even when a request supplies a loopback Host header.

Fetch-handler errors return a non-success status and JSON with `code`, `error`,
and `manual_excerpt_available: true`. The earlier browser middleware can return
its existing guard error shape. Clients must preserve manual excerpt entry for
**any** failed request, including guard errors and unavailable endpoints, and
must not report a URL as successfully read when no preview was returned.

| Code | HTTP status | Meaning |
|---|---:|---|
| `invalid_request` | JSON rejection status | Request is not the URL-only JSON contract |
| `invalid_url` | 400 | Unsupported or malformed URL |
| `blocked_destination` | 403 | Destination fails the public-address policy |
| `local_only` | 403 | Daemon bind scope is external or unparseable |
| `busy` | 429 | Fetch slots are full |
| `dns_failed` | 502 | Resolution failed, exceeded the address cap, or DNS slots are full |
| `too_many_redirects` | 502 | More than three redirects |
| `unsupported_content_type` | 415 | Unsupported MIME, charset, or invalid UTF-8 |
| `unsupported_encoding` | 415 | Response was compressed |
| `response_too_large` | 413 | Body, extracted text, or parsed node limit exceeded |
| `timeout` | 504 | Retrieval deadline expired |
| `fetch_failed` | 502 | Transport or parser task failed |
| `http_error` | 502 | Upstream did not return HTTP 200 |
| `no_readable_text` | 422 | Insufficient useful text |
