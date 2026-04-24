//! Web fetch and search tool implementations.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;

use crate::types::{
    SearchHit, WebFetchInput, WebFetchOutput, WebSearchInput, WebSearchOutput, WebSearchResultItem,
};

/// Monotonic counter used to generate unique `tool_use_id` values for web
/// search results within a single process lifetime.
static WEB_SEARCH_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Fetch a URL and return a summarized `WebFetchOutput`.
pub(crate) fn execute_web_fetch(input: &WebFetchInput) -> Result<WebFetchOutput, String> {
    let started = Instant::now();
    let client = build_http_client()?;
    let request_url = normalize_fetch_url(&input.url)?;
    let response = client
        .get(request_url.clone())
        .send()
        .map_err(|error| error.to_string())?;

    let status = response.status();
    let final_url = response.url().to_string();
    let code = status.as_u16();
    let code_text = status.canonical_reason().unwrap_or("Unknown").to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.text().map_err(|error| error.to_string())?;
    let bytes = body.len();
    let normalized = normalize_fetched_content(&body, &content_type);
    let result = summarize_web_fetch(&final_url, &input.prompt, &normalized, &body, &content_type);

    Ok(WebFetchOutput {
        bytes,
        code,
        code_text,
        result,
        duration_ms: started.elapsed().as_millis(),
        url: final_url,
    })
}

/// Search `DuckDuckGo` and return extracted hits.
pub(crate) fn execute_web_search(input: &WebSearchInput) -> Result<WebSearchOutput, String> {
    let started = Instant::now();
    let client = build_http_client()?;
    let search_url = build_search_url(&input.query)?;
    let response = client
        .get(search_url)
        .send()
        .map_err(|error| error.to_string())?;

    let final_url = response.url().clone();
    let html = response.text().map_err(|error| error.to_string())?;
    let mut hits = extract_search_hits(&html);

    if hits.is_empty() && final_url.host_str().is_some() {
        hits = extract_search_hits_from_generic_links(&html);
    }

    if let Some(allowed) = input.allowed_domains.as_ref() {
        hits.retain(|hit| host_matches_list(&hit.url, allowed));
    }
    if let Some(blocked) = input.blocked_domains.as_ref() {
        hits.retain(|hit| !host_matches_list(&hit.url, blocked));
    }

    dedupe_hits(&mut hits);
    hits.truncate(8);

    let summary = if hits.is_empty() {
        format!("No web search results matched the query {:?}.", input.query)
    } else {
        let rendered_hits = hits
            .iter()
            .map(|hit| format!("- [{}]({})", hit.title, hit.url))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Search results for {:?}. Include a Sources section in the final answer.\n{}",
            input.query, rendered_hits
        )
    };

    Ok(WebSearchOutput {
        query: input.query.clone(),
        results: vec![
            WebSearchResultItem::Commentary(summary),
            WebSearchResultItem::SearchResult {
                tool_use_id: format!(
                    "web_search_{}",
                    WEB_SEARCH_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
                ),
                content: hits,
            },
        ],
        duration_seconds: started.elapsed().as_secs_f64(),
    })
}

/// Build the shared blocking HTTP client.
pub(crate) fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("colotcook-rust-tools/0.1")
        .build()
        .map_err(|error| error.to_string())
}

/// Ensure the URL has a scheme (adds `https://` if missing).
pub(crate) fn normalize_fetch_url(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    if parsed.scheme() == "http" {
        let host = parsed.host_str().unwrap_or_default();
        if host != "localhost" && host != "127.0.0.1" && host != "::1" {
            let mut upgraded = parsed;
            upgraded
                .set_scheme("https")
                .map_err(|()| String::from("failed to upgrade URL to https"))?;
            return Ok(upgraded.to_string());
        }
    }
    Ok(parsed.to_string())
}

/// Build the `DuckDuckGo` lite search URL for a query.
pub(crate) fn build_search_url(query: &str) -> Result<reqwest::Url, String> {
    if let Ok(base) = std::env::var("COLOTCOOK_WEB_SEARCH_BASE_URL") {
        let mut url = reqwest::Url::parse(&base).map_err(|error| error.to_string())?;
        url.query_pairs_mut().append_pair("q", query);
        return Ok(url);
    }

    let mut url = reqwest::Url::parse("https://html.duckduckgo.com/html/")
        .map_err(|error| error.to_string())?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

/// Convert fetched body to plain text based on content-type.
pub(crate) fn normalize_fetched_content(body: &str, content_type: &str) -> String {
    if content_type.contains("html") {
        html_to_text(body)
    } else {
        body.trim().to_string()
    }
}

/// Build the result string shown to the model after fetching.
pub(crate) fn summarize_web_fetch(
    url: &str,
    prompt: &str,
    content: &str,
    raw_body: &str,
    content_type: &str,
) -> String {
    let lower_prompt = prompt.to_lowercase();
    let compact = collapse_whitespace(content);

    let detail = if lower_prompt.contains("title") {
        extract_title(content, raw_body, content_type).map_or_else(
            || preview_text(&compact, 600),
            |title| format!("Title: {title}"),
        )
    } else if lower_prompt.contains("summary") || lower_prompt.contains("summarize") {
        preview_text(&compact, 900)
    } else {
        let preview = preview_text(&compact, 900);
        format!("Prompt: {prompt}\nContent preview:\n{preview}")
    };

    format!("Fetched {url}\n{detail}")
}

/// Extract a page title from HTML or plain text content.
pub(crate) fn extract_title(content: &str, raw_body: &str, content_type: &str) -> Option<String> {
    if content_type.contains("html") {
        let lowered = raw_body.to_lowercase();
        if let Some(start) = lowered.find("<title>") {
            let after = start + "<title>".len();
            if let Some(end_rel) = lowered[after..].find("</title>") {
                let title =
                    collapse_whitespace(&decode_html_entities(&raw_body[after..after + end_rel]));
                if !title.is_empty() {
                    return Some(title);
                }
            }
        }
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Strip HTML tags and decode entities to produce readable text.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut previous_was_space = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            '&' => {
                text.push('&');
                previous_was_space = false;
            }
            ch if ch.is_whitespace() => {
                if !previous_was_space {
                    text.push(' ');
                    previous_was_space = true;
                }
            }
            _ => {
                text.push(ch);
                previous_was_space = false;
            }
        }
    }

    collapse_whitespace(&decode_html_entities(&text))
}

/// Replace common HTML entities with their UTF-8 equivalents.
pub(crate) fn decode_html_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Collapse consecutive whitespace characters into a single space.
pub(crate) fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate text to `max_chars`, appending `…` if cut.
pub(crate) fn preview_text(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let shortened = input.chars().take(max_chars).collect::<String>();
    format!("{}…", shortened.trim_end())
}

/// Extract search result hits from `DuckDuckGo` HTML.
pub(crate) fn extract_search_hits(html: &str) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut remaining = html;

    while let Some(anchor_start) = remaining.find("result__a") {
        let after_class = &remaining[anchor_start..];
        let Some(href_idx) = after_class.find("href=") else {
            remaining = &after_class[1..];
            continue;
        };
        let href_slice = &after_class[href_idx + 5..];
        let Some((url, rest)) = extract_quoted_value(href_slice) else {
            remaining = &after_class[1..];
            continue;
        };
        let Some(close_tag_idx) = rest.find('>') else {
            remaining = &after_class[1..];
            continue;
        };
        let after_tag = &rest[close_tag_idx + 1..];
        let Some(end_anchor_idx) = after_tag.find("</a>") else {
            remaining = &after_tag[1..];
            continue;
        };
        let title = html_to_text(&after_tag[..end_anchor_idx]);
        if let Some(decoded_url) = decode_duckduckgo_redirect(&url) {
            hits.push(SearchHit {
                title: title.trim().to_string(),
                url: decoded_url,
            });
        }
        remaining = &after_tag[end_anchor_idx + 4..];
    }

    hits
}

/// Extract search result hits from `DuckDuckGo` HTML.
pub(crate) fn extract_search_hits_from_generic_links(html: &str) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut remaining = html;

    while let Some(anchor_start) = remaining.find("<a") {
        let after_anchor = &remaining[anchor_start..];
        let Some(href_idx) = after_anchor.find("href=") else {
            remaining = &after_anchor[2..];
            continue;
        };
        let href_slice = &after_anchor[href_idx + 5..];
        let Some((url, rest)) = extract_quoted_value(href_slice) else {
            remaining = &after_anchor[2..];
            continue;
        };
        let Some(close_tag_idx) = rest.find('>') else {
            remaining = &after_anchor[2..];
            continue;
        };
        let after_tag = &rest[close_tag_idx + 1..];
        let Some(end_anchor_idx) = after_tag.find("</a>") else {
            remaining = &after_anchor[2..];
            continue;
        };
        let title = html_to_text(&after_tag[..end_anchor_idx]);
        if title.trim().is_empty() {
            remaining = &after_tag[end_anchor_idx + 4..];
            continue;
        }
        let decoded_url = decode_duckduckgo_redirect(&url).unwrap_or(url);
        if decoded_url.starts_with("http://") || decoded_url.starts_with("https://") {
            hits.push(SearchHit {
                title: title.trim().to_string(),
                url: decoded_url,
            });
        }
        remaining = &after_tag[end_anchor_idx + 4..];
    }

    hits
}

/// Extract a double-quoted value from the start of `input`.
pub(crate) fn extract_quoted_value(input: &str) -> Option<(String, &str)> {
    let quote = input.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &input[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some((rest[..end].to_string(), &rest[end + quote.len_utf8()..]))
}

/// Decode a `DuckDuckGo` redirect URL to the real destination.
pub(crate) fn decode_duckduckgo_redirect(url: &str) -> Option<String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(html_entity_decode_url(url));
    }

    let joined = if url.starts_with("//") {
        format!("https:{url}")
    } else if url.starts_with('/') {
        format!("https://duckduckgo.com{url}")
    } else {
        return None;
    };

    let parsed = reqwest::Url::parse(&joined).ok()?;
    if parsed.path() == "/l/" || parsed.path() == "/l" {
        for (key, value) in parsed.query_pairs() {
            if key == "uddg" {
                return Some(html_entity_decode_url(value.as_ref()));
            }
        }
    }
    Some(joined)
}

/// Decode `&amp;` entities inside a URL string.
pub(crate) fn html_entity_decode_url(url: &str) -> String {
    decode_html_entities(url)
}

/// Check whether a URL's host matches any domain in `domains`.
pub(crate) fn host_matches_list(url: &str, domains: &[String]) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    domains.iter().any(|domain| {
        let normalized = normalize_domain_filter(domain);
        !normalized.is_empty() && (host == normalized || host.ends_with(&format!(".{normalized}")))
    })
}

/// Normalize a domain filter string (strip scheme and trailing slash).
pub(crate) fn normalize_domain_filter(domain: &str) -> String {
    let trimmed = domain.trim();
    let candidate = reqwest::Url::parse(trimmed)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| trimmed.to_string());
    candidate
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// Remove duplicate search hits by URL.
pub(crate) fn dedupe_hits(hits: &mut Vec<SearchHit>) {
    let mut seen = BTreeSet::new();
    hits.retain(|hit| seen.insert(hit.url.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- normalize_fetch_url ---

    #[test]
    fn normalize_fetch_url_https_unchanged() {
        let url = normalize_fetch_url("https://example.com/").unwrap();
        assert!(url.starts_with("https://"));
    }

    #[test]
    fn normalize_fetch_url_http_upgraded_to_https() {
        let url = normalize_fetch_url("http://example.com/").unwrap();
        assert!(
            url.starts_with("https://"),
            "expected https upgrade, got: {url}"
        );
    }

    #[test]
    fn normalize_fetch_url_http_localhost_stays_http() {
        let url = normalize_fetch_url("http://localhost:8080/").unwrap();
        assert!(
            url.starts_with("http://"),
            "localhost should stay http, got: {url}"
        );
    }

    #[test]
    fn normalize_fetch_url_http_127_0_0_1_stays_http() {
        let url = normalize_fetch_url("http://127.0.0.1:3000/").unwrap();
        assert!(url.starts_with("http://"));
    }

    #[test]
    fn normalize_fetch_url_invalid_errors() {
        let result = normalize_fetch_url("not-a-url");
        assert!(result.is_err());
    }

    // --- build_search_url ---

    #[test]
    fn build_search_url_contains_query() {
        let url = build_search_url("rust lang").unwrap();
        let query = url.query().unwrap_or("");
        assert!(query.contains("rust"), "query params: {query}");
    }

    #[test]
    fn build_search_url_default_uses_duckduckgo() {
        std::env::remove_var("COLOTCOOK_WEB_SEARCH_BASE_URL");
        let url = build_search_url("test query").unwrap();
        assert!(
            url.host_str() == Some("html.duckduckgo.com"),
            "host: {:?}",
            url.host_str()
        );
    }

    #[test]
    fn build_search_url_env_override() {
        std::env::set_var(
            "COLOTCOOK_WEB_SEARCH_BASE_URL",
            "https://mysearch.example.com/",
        );
        let url = build_search_url("my query").unwrap();
        assert_eq!(url.host_str(), Some("mysearch.example.com"));
        std::env::remove_var("COLOTCOOK_WEB_SEARCH_BASE_URL");
    }

    // --- html_to_text ---

    #[test]
    fn html_to_text_strips_tags() {
        let result = html_to_text("<p>Hello <b>world</b></p>");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn html_to_text_empty_string() {
        let result = html_to_text("");
        assert_eq!(result, "");
    }

    #[test]
    fn html_to_text_no_tags_passthrough() {
        let result = html_to_text("plain text");
        assert_eq!(result, "plain text");
    }

    #[test]
    fn html_to_text_collapses_whitespace() {
        let result = html_to_text("<div>  hello   world  </div>");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn html_to_text_decodes_entities() {
        let result = html_to_text("<p>a &amp; b &lt;3&gt;</p>");
        assert!(result.contains('&'));
        assert!(result.contains('<'));
    }

    // --- decode_html_entities ---

    #[test]
    fn decode_html_entities_amp() {
        assert_eq!(decode_html_entities("a &amp; b"), "a & b");
    }

    #[test]
    fn decode_html_entities_lt_gt() {
        assert_eq!(decode_html_entities("&lt;div&gt;"), "<div>");
    }

    #[test]
    fn decode_html_entities_quot() {
        assert_eq!(decode_html_entities("&quot;hello&quot;"), "\"hello\"");
    }

    #[test]
    fn decode_html_entities_apos() {
        assert_eq!(decode_html_entities("it&#39;s"), "it's");
    }

    #[test]
    fn decode_html_entities_nbsp() {
        assert_eq!(decode_html_entities("a&nbsp;b"), "a b");
    }

    #[test]
    fn decode_html_entities_no_entities_unchanged() {
        assert_eq!(decode_html_entities("hello world"), "hello world");
    }

    // --- collapse_whitespace ---

    #[test]
    fn collapse_whitespace_multiple_spaces() {
        assert_eq!(collapse_whitespace("a  b   c"), "a b c");
    }

    #[test]
    fn collapse_whitespace_tabs_and_newlines() {
        assert_eq!(collapse_whitespace("a\t\nb"), "a b");
    }

    #[test]
    fn collapse_whitespace_empty_string() {
        assert_eq!(collapse_whitespace(""), "");
    }

    #[test]
    fn collapse_whitespace_leading_trailing() {
        assert_eq!(collapse_whitespace("  hello  "), "hello");
    }

    // --- normalize_domain_filter ---

    #[test]
    fn normalize_domain_filter_plain_domain() {
        assert_eq!(normalize_domain_filter("example.com"), "example.com");
    }

    #[test]
    fn normalize_domain_filter_with_scheme() {
        assert_eq!(
            normalize_domain_filter("https://example.com/"),
            "example.com"
        );
    }

    #[test]
    fn normalize_domain_filter_uppercase_lowercased() {
        assert_eq!(normalize_domain_filter("Example.COM"), "example.com");
    }

    #[test]
    fn normalize_domain_filter_leading_dot_stripped() {
        assert_eq!(normalize_domain_filter(".example.com"), "example.com");
    }

    #[test]
    fn normalize_domain_filter_trailing_slash_stripped() {
        assert_eq!(normalize_domain_filter("example.com/"), "example.com");
    }

    #[test]
    fn normalize_domain_filter_empty_string() {
        assert_eq!(normalize_domain_filter(""), "");
    }

    // --- dedupe_hits ---

    #[test]
    fn dedupe_hits_removes_duplicates() {
        let mut hits = vec![
            SearchHit {
                title: String::from("A"),
                url: String::from("https://a.com"),
            },
            SearchHit {
                title: String::from("B"),
                url: String::from("https://a.com"),
            },
            SearchHit {
                title: String::from("C"),
                url: String::from("https://b.com"),
            },
        ];
        dedupe_hits(&mut hits);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://a.com");
        assert_eq!(hits[1].url, "https://b.com");
    }

    #[test]
    fn dedupe_hits_no_duplicates_unchanged() {
        let mut hits = vec![
            SearchHit {
                title: String::from("A"),
                url: String::from("https://a.com"),
            },
            SearchHit {
                title: String::from("B"),
                url: String::from("https://b.com"),
            },
        ];
        dedupe_hits(&mut hits);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn dedupe_hits_empty_vec() {
        let mut hits: Vec<SearchHit> = vec![];
        dedupe_hits(&mut hits);
        assert!(hits.is_empty());
    }

    // --- host_matches_list ---

    #[test]
    fn host_matches_list_exact_match() {
        let domains = vec![String::from("example.com")];
        assert!(host_matches_list("https://example.com/path", &domains));
    }

    #[test]
    fn host_matches_list_subdomain_match() {
        let domains = vec![String::from("example.com")];
        assert!(host_matches_list("https://sub.example.com/", &domains));
    }

    #[test]
    fn host_matches_list_no_match() {
        let domains = vec![String::from("example.com")];
        assert!(!host_matches_list("https://other.com/", &domains));
    }

    #[test]
    fn host_matches_list_invalid_url() {
        let domains = vec![String::from("example.com")];
        assert!(!host_matches_list("not-a-url", &domains));
    }

    #[test]
    fn host_matches_list_empty_domains() {
        let domains: Vec<String> = vec![];
        assert!(!host_matches_list("https://example.com/", &domains));
    }

    // --- extract_quoted_value ---

    #[test]
    fn extract_quoted_value_double_quoted() {
        let result = extract_quoted_value("\"hello\" rest");
        assert_eq!(result, Some((String::from("hello"), " rest")));
    }

    #[test]
    fn extract_quoted_value_single_quoted() {
        let result = extract_quoted_value("'world' more");
        assert_eq!(result, Some((String::from("world"), " more")));
    }

    #[test]
    fn extract_quoted_value_no_quote_returns_none() {
        let result = extract_quoted_value("no quote here");
        assert!(result.is_none());
    }

    #[test]
    fn extract_quoted_value_empty_string() {
        let result = extract_quoted_value("");
        assert!(result.is_none());
    }

    // --- normalize_fetched_content ---

    #[test]
    fn normalize_fetched_content_html_strips_tags() {
        let result = normalize_fetched_content("<b>bold</b>", "text/html");
        assert_eq!(result, "bold");
    }

    #[test]
    fn normalize_fetched_content_json_passthrough() {
        let result = normalize_fetched_content(r#"{"key":"val"}"#, "application/json");
        assert_eq!(result, r#"{"key":"val"}"#);
    }

    #[test]
    fn normalize_fetched_content_plain_text_trimmed() {
        let result = normalize_fetched_content("  hello  ", "text/plain");
        assert_eq!(result, "hello");
    }
}
