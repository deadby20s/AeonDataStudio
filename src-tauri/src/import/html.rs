//! Main-text extraction from HTML, and reading WARC web archives (e.g. Common Crawl).

use std::io::BufRead;

use scraper::{ElementRef, Html, Node, Selector};

use crate::error::{AppError, AppResult};

/// Elements whose content is never document text.
const SKIP: [&str; 16] = [
    "script", "style", "noscript", "nav", "header", "footer", "aside", "form", "template", "svg", "iframe", "button",
    "select", "input", "textarea", "canvas",
];
/// Elements that start a new paragraph.
const BLOCK: [&str; 26] = [
    "p", "div", "section", "article", "main", "h1", "h2", "h3", "h4", "h5", "h6", "li", "ul", "ol", "table", "tr", "td",
    "th", "blockquote", "pre", "dd", "dt", "dl", "figcaption", "address", "hr",
];

pub struct Extracted {
    pub title: Option<String>,
    pub text: String,
}

fn hidden(e: &scraper::node::Element) -> bool {
    e.attr("hidden").is_some()
        || e.attr("aria-hidden") == Some("true")
        || e.attr("style").is_some_and(|s| s.replace(' ', "").contains("display:none"))
}

fn walk(el: ElementRef<'_>, out: &mut String, in_pre: bool) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                if in_pre {
                    out.push_str(t);
                } else {
                    let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    if collapsed.is_empty() {
                        if !t.is_empty() && !out.ends_with([' ', '\n']) && !out.is_empty() {
                            out.push(' ');
                        }
                        continue;
                    }
                    if t.starts_with(char::is_whitespace) && !out.ends_with([' ', '\n']) && !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(&collapsed);
                    if t.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                }
            }
            Node::Element(e) => {
                let name = e.name();
                if SKIP.contains(&name) || hidden(e) {
                    continue;
                }
                if name == "br" {
                    out.push('\n');
                    continue;
                }
                let block = BLOCK.contains(&name);
                if block {
                    out.push_str("\n\n");
                }
                if let Some(child_el) = ElementRef::wrap(child) {
                    walk(child_el, out, in_pre || name == "pre");
                }
                if block {
                    out.push_str("\n\n");
                }
            }
            _ => {}
        }
    }
}

fn tidy(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank = 0;
    for line in raw.lines().map(str::trim) {
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        out.push_str(line);
        blank = 0;
    }
    out
}

/// Extracts the readable text of a page. Prefers `<article>` or `<main>` when they hold most
/// of the text; otherwise uses `<body>` without navigation, headers, footers and the like.
pub fn extract(html: &str) -> Extracted {
    let doc = Html::parse_document(html);
    let sel = |s: &str| Selector::parse(s).expect("valid selector");
    let title = doc
        .select(&sel("title"))
        .next()
        .map(|t| t.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty());

    let text_of = |el: ElementRef<'_>| {
        let mut buf = String::new();
        walk(el, &mut buf, false);
        tidy(&buf)
    };
    let body = doc.select(&sel("body")).next().map(text_of).unwrap_or_else(|| text_of(doc.root_element()));
    let focused = doc
        .select(&sel("article, main"))
        .map(text_of)
        .max_by_key(|t| t.len())
        .filter(|t| t.len() >= 200 && t.len() * 2 >= body.len());
    Extracted { title, text: focused.unwrap_or(body) }
}

/// One HTML page from a WARC file.
pub struct WarcPage {
    pub record_id: Option<String>,
    pub url: Option<String>,
    pub date: Option<String>,
    pub html: String,
}

fn read_line(r: &mut dyn BufRead, buf: &mut Vec<u8>) -> AppResult<bool> {
    buf.clear();
    Ok(r.read_until(b'\n', buf)? > 0)
}

/// Reads WARC records, calling `emit` for each successful (HTTP 200) HTML response.
/// Other records (requests, metadata, images, redirects) are skipped. Stops if `emit` returns
/// `Ok(false)`.
pub fn read_warc(r: &mut dyn BufRead, emit: &mut dyn FnMut(WarcPage) -> AppResult<bool>) -> AppResult<()> {
    let mut line = Vec::new();
    loop {
        // Find the next record start (skipping blank lines between records).
        let mut found = false;
        while read_line(r, &mut line)? {
            if line.starts_with(b"WARC/") {
                found = true;
                break;
            }
        }
        if !found {
            return Ok(());
        }
        let (mut length, mut kind, mut id, mut url, mut date) = (None, String::new(), None, None, None);
        while read_line(r, &mut line)? {
            let l = String::from_utf8_lossy(&line);
            let l = l.trim_end();
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                let v = v.trim().to_string();
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-length" => length = v.parse::<usize>().ok(),
                    "warc-type" => kind = v.to_ascii_lowercase(),
                    "warc-record-id" => id = Some(v.trim_matches(['<', '>']).to_string()),
                    "warc-target-uri" => url = Some(v),
                    "warc-date" => date = Some(v),
                    _ => {}
                }
            }
        }
        let length = length.ok_or_else(|| AppError::InvalidInput("WARC record without Content-Length".into()))?;
        let mut block = vec![0u8; length];
        r.read_exact(&mut block)?;
        if kind != "response" {
            continue;
        }
        // The block is an HTTP response: status line, headers, blank line, body.
        let Some(split) = block.windows(4).position(|w| w == b"\r\n\r\n").map(|p| (p, 4)).or_else(|| {
            block.windows(2).position(|w| w == b"\n\n").map(|p| (p, 2))
        }) else {
            continue;
        };
        let head = String::from_utf8_lossy(&block[..split.0]).to_ascii_lowercase();
        let ok = head.lines().next().is_some_and(|s| s.split_whitespace().nth(1) == Some("200"));
        let is_html = head.lines().any(|l| l.starts_with("content-type:") && l.contains("html"));
        if !ok || !is_html {
            continue;
        }
        let html = String::from_utf8_lossy(&block[split.0 + split.1..]).into_owned();
        if !emit(WarcPage { record_id: id, url, date, html })? {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html><head><title> Gardening  tips </title><style>p{color:red}</style></head>
<body><header><a href="/">Home</a> | <a href="/about">About</a></header>
<nav><ul><li>Menu item</li></ul></nav>
<main><h1>Growing tomatoes</h1><p>Tomatoes need <b>full</b> sun and   regular water.</p>
<p>Prune the side shoots<br>every week.</p><div hidden>secret</div>
<pre>  keep   spacing</pre><script>var x = 1;</script>
<ul><li>Seeds</li><li>Compost</li></ul></main>
<footer>© 2026 Example</footer></body></html>"#;

    #[test]
    fn extracts_main_text_without_boilerplate() {
        let e = extract(PAGE);
        assert_eq!(e.title.as_deref(), Some("Gardening tips"));
        assert_eq!(
            e.text,
            "Growing tomatoes\n\nTomatoes need full sun and regular water.\n\nPrune the side shoots\nevery week.\n\nkeep   spacing\n\nSeeds\n\nCompost"
        );
        for junk in ["Menu item", "About", "secret", "var x", "©", "color:red"] {
            assert!(!e.text.contains(junk), "{junk}");
        }
    }

    #[test]
    fn reads_html_responses_from_warc() {
        let http = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{PAGE}");
        let redirect = "HTTP/1.1 301 Moved\r\nContent-Type: text/html\r\n\r\n<p>moved</p>";
        let mut warc = String::new();
        for (kind, uri, body) in [
            ("warcinfo", "", "software: test\r\n".to_string()),
            ("request", "https://example.com/", "GET / HTTP/1.1\r\n\r\n".to_string()),
            ("response", "https://example.com/tomatoes", http.clone()),
            ("response", "https://example.com/old", redirect.to_string()),
        ] {
            warc.push_str(&format!(
                "WARC/1.0\r\nWARC-Type: {kind}\r\nWARC-Record-ID: <urn:uuid:{kind}-1>\r\nWARC-Target-URI: {uri}\r\nWARC-Date: 2026-11-17T10:00:00Z\r\nContent-Length: {}\r\n\r\n{body}\r\n\r\n",
                body.len()
            ));
        }
        let mut pages = Vec::new();
        read_warc(&mut warc.as_bytes(), &mut |p| {
            pages.push(p);
            Ok(true)
        })
        .unwrap();
        assert_eq!(pages.len(), 1, "only the 200 HTML response");
        assert_eq!(pages[0].url.as_deref(), Some("https://example.com/tomatoes"));
        assert_eq!(pages[0].record_id.as_deref(), Some("urn:uuid:response-1"));
        assert!(extract(&pages[0].html).text.starts_with("Growing tomatoes"));
    }
}
