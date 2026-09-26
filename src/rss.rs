use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::bencode::{self, Value};
use crate::xml;

#[derive(Clone)]
pub struct FeedItem {
    pub title: String,
    pub link: String,
    pub is_torrent: bool,
    pub guid: String,
}

#[derive(Clone)]
pub struct RssFeed {
    pub url: String,
    pub title: String,
    pub items: Vec<FeedItem>,
    pub last_poll: u64,
    pub poll_interval_secs: u64,
}

#[derive(Clone)]
pub struct RssRule {
    pub name: String,
    pub feed_url: String,
    pub pattern: String,
}

pub struct RssState {
    pub feeds: Vec<RssFeed>,
    pub rules: Vec<RssRule>,
    pub seen_guids: Vec<String>,
}

pub const MAX_SEEN_GUIDS: usize = 50_000;
const MAX_RSS_STATE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_RSS_FEEDS: usize = 1_024;
pub(crate) const MAX_RSS_RULES: usize = 1_024;
pub(crate) const MAX_RSS_TEXT_BYTES: usize = 8 * 1024;
pub(crate) const MAX_RSS_PATTERN_BYTES: usize = 512;
const MAX_FEED_ITEMS: usize = 2_000;

impl RssState {
    pub fn new() -> Self {
        Self {
            feeds: Vec::new(),
            rules: Vec::new(),
            seen_guids: Vec::new(),
        }
    }
}

pub fn parse_feed(data: &[u8]) -> Result<(String, Vec<FeedItem>), String> {
    let root = xml::parse(data).ok_or_else(|| "invalid xml".to_string())?;
    match root.local_name() {
        name if name.eq_ignore_ascii_case("rss") => parse_rss(&root),
        name if name.eq_ignore_ascii_case("feed") => parse_atom(&root),
        name if name.eq_ignore_ascii_case("rdf") => parse_rdf(&root),
        _ => Err(format!("unknown feed root tag: {}", root.tag)),
    }
}

fn parse_rss(root: &xml::XmlNode) -> Result<(String, Vec<FeedItem>), String> {
    let channel = root.child("channel").ok_or("missing <channel>")?;
    Ok((
        text(channel, "title").to_string(),
        parse_rss_items(channel.children_by_tag("item")),
    ))
}

fn parse_rdf(root: &xml::XmlNode) -> Result<(String, Vec<FeedItem>), String> {
    let title = root
        .child("channel")
        .map_or("", |channel| text(channel, "title"));
    Ok((
        title.to_string(),
        parse_rss_items(root.children_by_tag("item")),
    ))
}

/// Trimmed text of the first `tag` child, or "".
fn text<'a>(node: &'a xml::XmlNode, tag: &str) -> &'a str {
    node.child(tag).map_or("", |child| child.text.trim())
}

/// Build an item, using the link as GUID when none is given. Items without
/// a link or with oversized fields are dropped.
fn feed_item(title: &str, link: &str, is_torrent: bool, guid: &str) -> Option<FeedItem> {
    let guid = if guid.is_empty() { link } else { guid };
    if link.is_empty()
        || [title, link, guid]
            .iter()
            .any(|v| v.len() > MAX_RSS_TEXT_BYTES)
    {
        return None;
    }
    Some(FeedItem {
        title: title.to_string(),
        link: link.to_string(),
        is_torrent,
        guid: guid.to_string(),
    })
}

fn parse_rss_items(item_nodes: Vec<&xml::XmlNode>) -> Vec<FeedItem> {
    item_nodes
        .into_iter()
        .take(MAX_FEED_ITEMS)
        .filter_map(|item| {
            let enclosure = item
                .children_by_tag("enclosure")
                .into_iter()
                .find_map(|node| {
                    let url = node.attr("url")?.trim();
                    let content_type = node.attr("type").unwrap_or("");
                    (!url.is_empty()
                        && (is_torrent_url(url) || is_torrent_content_type(content_type)))
                    .then_some(url)
                });
            let magnet = Some(text(item, "magnetURI")).filter(|value| is_torrent_url(value));
            let link = text(item, "link");
            let (link, is_torrent) = match enclosure.or(magnet) {
                Some(url) => (url, true),
                None => (link, is_torrent_url(link)),
            };
            feed_item(text(item, "title"), link, is_torrent, text(item, "guid"))
        })
        .collect()
}

fn parse_atom(root: &xml::XmlNode) -> Result<(String, Vec<FeedItem>), String> {
    let items = root
        .children_by_tag("entry")
        .into_iter()
        .take(MAX_FEED_ITEMS)
        .filter_map(|entry| {
            let links = entry.children_by_tag("link");
            let preferred = links.iter().find_map(|node| {
                let href = node.attr("href")?.trim();
                let rel = node.attr("rel").unwrap_or("");
                let content_type = node.attr("type").unwrap_or("");
                (!href.is_empty()
                    && rel.eq_ignore_ascii_case("enclosure")
                    && (is_torrent_url(href) || is_torrent_content_type(content_type)))
                .then_some((href, true))
            });
            let torrent_link = links.iter().find_map(|node| {
                let href = node.attr("href")?.trim();
                is_torrent_url(href).then_some((href, true))
            });
            let fallback = links.iter().find_map(|node| {
                let href = node.attr("href")?.trim();
                let rel = node.attr("rel").unwrap_or("alternate");
                (!href.is_empty() && rel.eq_ignore_ascii_case("alternate"))
                    .then(|| (href, is_torrent_url(href)))
            });
            let (link, is_torrent) = preferred.or(torrent_link).or(fallback).unwrap_or_default();
            feed_item(text(entry, "title"), link, is_torrent, text(entry, "id"))
        })
        .collect();
    Ok((text(root, "title").to_string(), items))
}

fn is_torrent_content_type(value: &str) -> bool {
    value
        .as_bytes()
        .windows(10)
        .any(|window| window.eq_ignore_ascii_case(b"bittorrent"))
}

fn is_torrent_url(value: &str) -> bool {
    let value = value.trim();
    if is_magnet_link(value) {
        return true;
    }
    let path = value.split(['#', '?']).next().unwrap_or(value).as_bytes();
    path.len() >= 8 && path[path.len() - 8..].eq_ignore_ascii_case(b".torrent")
}

pub fn is_magnet_link(value: &str) -> bool {
    value
        .trim()
        .get(..8)
        .map(|prefix| prefix.eq_ignore_ascii_case("magnet:?"))
        .unwrap_or(false)
}

pub fn seen_key(feed_url: &str, guid: &str) -> String {
    let mut digest = crate::sha256::Sha256::new();
    digest.update(&(feed_url.len() as u64).to_be_bytes());
    digest.update(feed_url.as_bytes());
    digest.update(guid.as_bytes());
    let mut key = String::from("v3:");
    for byte in digest.finalize() {
        for nibble in [byte >> 4, byte & 15] {
            key.push(char::from(b"0123456789abcdef"[usize::from(nibble)]));
        }
    }
    key
}

fn legacy_scoped_seen_key(feed_url: &str, guid: &str) -> String {
    format!("v2:{}:{feed_url}:{guid}", feed_url.len())
}

pub fn remember_seen(seen: &mut Vec<String>, key: String) {
    if seen.iter().any(|existing| existing == &key) {
        return;
    }
    seen.push(key);
    if seen.len() > MAX_SEEN_GUIDS {
        let remove = seen.len() - MAX_SEEN_GUIDS;
        seen.drain(..remove);
    }
}

pub fn match_rules<'a>(
    items: &'a [FeedItem],
    rules: &'a [RssRule],
    seen: &[String],
    feed_url: &str,
) -> Vec<(&'a FeedItem, &'a RssRule)> {
    let mut matches = Vec::new();
    let seen = seen.iter().map(String::as_str).collect::<HashSet<_>>();
    for item in items {
        let scoped_guid = seen_key(feed_url, &item.guid);
        let legacy_scoped_guid = legacy_scoped_seen_key(feed_url, &item.guid);
        if seen.contains(item.guid.as_str())
            || seen.contains(scoped_guid.as_str())
            || seen.contains(legacy_scoped_guid.as_str())
        {
            continue;
        }
        if !item.is_torrent {
            continue;
        }
        for rule in rules {
            if !rule.feed_url.is_empty() && rule.feed_url != feed_url {
                continue;
            }
            if glob_match(&rule.pattern, &item.title) {
                matches.push((item, rule));
                break;
            }
        }
    }
    matches
}

fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let text_lower = text.to_ascii_lowercase();
    for alt in pattern.split('|') {
        let alt = alt.trim().to_ascii_lowercase();
        if alt.is_empty() {
            continue;
        }
        if glob_match_single(&alt, &text_lower) {
            return true;
        }
    }
    false
}

fn glob_match_single(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return text.contains(pattern);
    }
    let mut pos = 0;
    for (idx, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if let Some(found) = text[pos..].find(part) {
            if idx == 0 && found != 0 {
                return false;
            }
            pos += found + part.len();
        } else {
            return false;
        }
    }
    if let Some(last) = parts.last() {
        if !last.is_empty() && !text.ends_with(last) {
            return false;
        }
    }
    true
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn save_rss_state(path: &Path, state: &RssState) -> Result<(), String> {
    if state.feeds.len() > MAX_RSS_FEEDS || state.rules.len() > MAX_RSS_RULES {
        return Err("rss save: too many feeds or rules".to_string());
    }
    if state
        .feeds
        .iter()
        .any(|feed| feed.url.len() > MAX_RSS_TEXT_BYTES || feed.title.len() > MAX_RSS_TEXT_BYTES)
        || state.rules.iter().any(|rule| {
            rule.name.len() > MAX_RSS_TEXT_BYTES
                || rule.feed_url.len() > MAX_RSS_TEXT_BYTES
                || rule.pattern.len() > MAX_RSS_PATTERN_BYTES
        })
    {
        return Err("rss save: feed or rule text is too large".to_string());
    }
    if state.seen_guids.len() > MAX_SEEN_GUIDS {
        return Err("rss save: too many seen entries".to_string());
    }
    // Encode directly; keys are written in bencode (sorted) order. The
    // bounds above keep the value count far below the parser's limits.
    let mut data = Vec::with_capacity(4096 + state.seen_guids.len() * 72);
    data.extend_from_slice(b"d5:feedsl");
    for feed in &state.feeds {
        data.extend_from_slice(b"d9:last_poll");
        put_int(&mut data, feed.last_poll);
        data.extend_from_slice(b"13:poll_interval");
        put_int(&mut data, feed.poll_interval_secs);
        put_bytes(&mut data, b"title");
        put_bytes(&mut data, feed.title.as_bytes());
        put_bytes(&mut data, b"url");
        put_bytes(&mut data, feed.url.as_bytes());
        data.push(b'e');
    }
    data.extend_from_slice(b"e5:rulesl");
    for rule in &state.rules {
        data.push(b'd');
        for (key, value) in [
            ("feed_url", &rule.feed_url),
            ("name", &rule.name),
            ("pattern", &rule.pattern),
        ] {
            put_bytes(&mut data, key.as_bytes());
            put_bytes(&mut data, value.as_bytes());
        }
        data.push(b'e');
    }
    data.extend_from_slice(b"e4:seenl");
    for guid in &state.seen_guids {
        put_bytes(&mut data, guid.as_bytes());
    }
    data.extend_from_slice(b"ee");
    write_atomic(path, &data, true)
}

pub fn load_rss_state(path: &Path) -> Result<RssState, String> {
    let (primary_error, primary_missing) =
        match crate::read_file_limited(path, MAX_RSS_STATE_BYTES, true) {
            Ok(data) => match parse_rss_state(&data) {
                Ok(state) => return Ok(state),
                Err(err) => (err, false),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                ("rss state file is missing".to_string(), true)
            }
            Err(err) => (format!("rss load: {err}"), false),
        };
    let backup = sidecar_path(path, ".bak");
    let backup_data = match crate::read_file_limited(&backup, MAX_RSS_STATE_BYTES, true) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && primary_missing => {
            return Err("rss load: state file not found".to_string());
        }
        Err(err) => return Err(format!("rss parse: {primary_error}; backup read: {err}")),
    };
    let state = parse_rss_state(&backup_data)
        .map_err(|backup_error| format!("rss parse: {primary_error}; backup: {backup_error}"))?;
    if let Err(err) = write_atomic(path, &backup_data, false) {
        eprintln!("warning: RSS backup loaded but primary restore failed: {err}");
    }
    Ok(state)
}

pub fn saved_state_exists(path: &Path) -> bool {
    #[cfg(any(unix, windows))]
    if crate::state_dir::is_state_file_path(path) {
        return crate::state_dir::exists(path).unwrap_or(false)
            || crate::state_dir::exists(&sidecar_path(path, ".bak")).unwrap_or(false);
    }
    path.exists() || sidecar_path(path, ".bak").exists()
}

fn parse_rss_state(data: &[u8]) -> Result<RssState, String> {
    let value = bencode::parse(data).map_err(|err| format!("rss parse: {err}"))?;
    let dict = match value {
        Value::Dict(items) => items,
        _ => return Err("rss state not a dict".to_string()),
    };
    let mut state = RssState::new();
    if let Some(Value::List(feeds)) = dict_get(&dict, b"feeds") {
        if feeds.len() > MAX_RSS_FEEDS {
            return Err("rss state has too many feeds".to_string());
        }
        for item in feeds {
            if let Value::Dict(fd) = item {
                let url = dict_get_str(fd, b"url").unwrap_or_default();
                let title = dict_get_str(fd, b"title").unwrap_or_default();
                if url.trim().is_empty()
                    || url.len() > MAX_RSS_TEXT_BYTES
                    || title.len() > MAX_RSS_TEXT_BYTES
                {
                    continue;
                }
                let last_poll = dict_get_int(fd, b"last_poll").unwrap_or(0).max(0) as u64;
                let poll_interval = dict_get_int(fd, b"poll_interval").unwrap_or(900).max(1) as u64;
                state.feeds.push(RssFeed {
                    url,
                    title,
                    items: Vec::new(),
                    last_poll,
                    poll_interval_secs: poll_interval,
                });
            }
        }
    }
    if let Some(Value::List(rules)) = dict_get(&dict, b"rules") {
        if rules.len() > MAX_RSS_RULES {
            return Err("rss state has too many rules".to_string());
        }
        for item in rules {
            if let Value::Dict(rd) = item {
                let name = dict_get_str(rd, b"name").unwrap_or_default();
                let feed_url = dict_get_str(rd, b"feed_url").unwrap_or_default();
                let pattern = dict_get_str(rd, b"pattern").unwrap_or_default();
                if !name.trim().is_empty()
                    && !pattern.trim().is_empty()
                    && name.len() <= MAX_RSS_TEXT_BYTES
                    && feed_url.len() <= MAX_RSS_TEXT_BYTES
                    && pattern.len() <= MAX_RSS_PATTERN_BYTES
                {
                    state.rules.push(RssRule {
                        name,
                        feed_url,
                        pattern,
                    });
                }
            }
        }
    }
    if let Some(Value::List(seen)) = dict_get(&dict, b"seen") {
        let mut unique = HashSet::with_capacity(seen.len().min(MAX_SEEN_GUIDS));
        for item in seen.iter().rev() {
            if state.seen_guids.len() >= MAX_SEEN_GUIDS {
                break;
            }
            if let Value::Bytes(bytes) = item {
                if bytes.is_empty() || bytes.len() > MAX_RSS_TEXT_BYTES {
                    continue;
                }
                if let Ok(s) = String::from_utf8(bytes.clone()) {
                    if unique.insert(s.clone()) {
                        state.seen_guids.push(s);
                    }
                }
            }
        }
        state.seen_guids.reverse();
    }
    Ok(state)
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    use std::io::Write;
    let _ = write!(out, "{}:", bytes.len());
    out.extend_from_slice(bytes);
}

fn put_int(out: &mut Vec<u8>, value: u64) {
    use std::io::Write;
    let _ = write!(out, "i{}e", value.min(i64::MAX as u64));
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

/// Atomically replace the state file, optionally keeping the previous
/// version as `.bak`. Uses the crate-wide writer: descriptor-pinned state
/// directory writes on Unix/Windows, and a no-follow temp+rename elsewhere.
fn write_atomic(path: &Path, data: &[u8], rotate_backup: bool) -> Result<(), String> {
    if data.len() > MAX_RSS_STATE_BYTES {
        return Err("rss save: state file is too large".to_string());
    }
    crate::write_atomic_file(path, data, "rss", rotate_backup, true)
}

fn dict_get<'a>(dict: &'a [(Vec<u8>, Value)], key: &[u8]) -> Option<&'a Value> {
    dict.iter()
        .find(|(k, _)| k.as_slice() == key)
        .map(|(_, v)| v)
}

fn dict_get_str(dict: &[(Vec<u8>, Value)], key: &[u8]) -> Option<String> {
    match dict_get(dict, key) {
        Some(Value::Bytes(bytes)) => String::from_utf8(bytes.clone()).ok(),
        _ => None,
    }
}

fn dict_get_int(dict: &[(Vec<u8>, Value)], key: &[u8]) -> Option<i64> {
    match dict_get(dict, key) {
        Some(Value::Int(n)) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustorrent-rss-{name}-{nanos}.benc"))
    }

    #[test]
    fn parse_rss_feed() {
        let xml = br#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <item>
      <title>Ubuntu ISO</title>
      <link>http://example.com/ubuntu.torrent</link>
      <guid>guid-001</guid>
    </item>
    <item>
      <title>Debian ISO</title>
      <enclosure url="http://example.com/debian.torrent" type="application/x-bittorrent"/>
      <guid>guid-002</guid>
    </item>
  </channel>
</rss>"#;
        let (title, items) = parse_feed(xml).unwrap();
        assert_eq!(title, "Test Feed");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Ubuntu ISO");
        assert!(items[0].is_torrent);
        assert_eq!(items[1].link, "http://example.com/debian.torrent");
        assert!(items[1].is_torrent);
    }

    #[test]
    fn parse_atom_feed() {
        let xml = br#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Feed</title>
  <entry>
    <title>Item One</title>
    <id>urn:uuid:001</id>
    <link href="http://example.com/1.torrent"/>
  </entry>
</feed>"#;
        let (title, items) = parse_feed(xml).unwrap();
        assert_eq!(title, "Atom Feed");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Item One");
        assert!(items[0].is_torrent);
        assert_eq!(items[0].guid, "urn:uuid:001");
    }

    #[test]
    fn atom_prefers_torrent_enclosure_over_alternate_link() {
        let xml = br#"<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Releases</title>
  <entry>
    <title>Release</title>
    <link rel="alternate" href="https://example.com/release"/>
    <link rel="enclosure" type="application/x-bittorrent" href="https://cdn.example.com/download?id=1"/>
  </entry>
</feed>"#;
        let (_, items) = parse_feed(xml).unwrap();
        assert_eq!(items[0].link, "https://cdn.example.com/download?id=1");
        assert!(items[0].is_torrent);
    }

    #[test]
    fn rss_recognizes_magnets_and_torrent_urls_with_queries() {
        let xml = br#"<rss><channel><title>Feed</title>
  <item><title>Magnet</title><link>magnet:?xt=urn:btih:abc</link></item>
  <item><title>File</title><link>https://example.com/file.TORRENT?token=1</link></item>
</channel></rss>"#;
        let (_, items) = parse_feed(xml).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.is_torrent));
    }

    #[test]
    fn parses_namespaced_atom_and_rdf_feeds() {
        let atom = br#"<atom:feed xmlns:atom="urn:atom"><atom:title>Atom</atom:title><atom:entry><atom:title>One</atom:title><atom:link href="magnet:?xt=urn:btih:abc"/></atom:entry></atom:feed>"#;
        let (title, items) = parse_feed(atom).unwrap();
        assert_eq!(title, "Atom");
        assert_eq!(items.len(), 1);

        let rdf = br#"<rdf:RDF xmlns:rdf="urn:rdf"><channel><title>RDF</title></channel><item><title>One</title><link>https://example.com/one.torrent</link></item></rdf:RDF>"#;
        let (title, items) = parse_feed(rdf).unwrap();
        assert_eq!(title, "RDF");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn glob_match_patterns() {
        assert!(glob_match("*ubuntu*", "Ubuntu 24.04 LTS"));
        assert!(glob_match("debian*", "debian-12.iso"));
        assert!(!glob_match("debian*", "Ubuntu 24.04"));
        assert!(glob_match("ubuntu|debian", "My Debian ISO"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("", "anything"));
    }

    #[test]
    fn match_rules_filters_seen_and_non_torrent() {
        let items = vec![
            FeedItem {
                title: "Ubuntu ISO".to_string(),
                link: "http://example.com/ubuntu.torrent".to_string(),
                is_torrent: true,
                guid: "guid-1".to_string(),
            },
            FeedItem {
                title: "Debian ISO".to_string(),
                link: "http://example.com/debian.torrent".to_string(),
                is_torrent: true,
                guid: "guid-2".to_string(),
            },
            FeedItem {
                title: "News article".to_string(),
                link: "http://example.com/news".to_string(),
                is_torrent: false,
                guid: "guid-3".to_string(),
            },
        ];
        let rules = vec![RssRule {
            name: "linux".to_string(),
            feed_url: String::new(),
            pattern: "*".to_string(),
        }];
        let seen = vec!["guid-1".to_string()];
        let matches = match_rules(&items, &rules, &seen, "http://feed");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0.title, "Debian ISO");
    }

    #[test]
    fn seen_guids_are_scoped_to_the_feed() {
        let item = FeedItem {
            title: "Release".to_string(),
            link: "magnet:?xt=urn:btih:abc".to_string(),
            is_torrent: true,
            guid: "shared-guid".to_string(),
        };
        let rule = RssRule {
            name: "all".to_string(),
            feed_url: String::new(),
            pattern: "*".to_string(),
        };
        let seen = vec![seen_key("https://feed-a.example/rss", &item.guid)];
        assert!(match_rules(
            std::slice::from_ref(&item),
            std::slice::from_ref(&rule),
            &seen,
            "https://feed-a.example/rss"
        )
        .is_empty());
        assert_eq!(
            match_rules(
                std::slice::from_ref(&item),
                std::slice::from_ref(&rule),
                &seen,
                "https://feed-b.example/rss"
            )
            .len(),
            1
        );
    }

    #[test]
    fn direct_state_encoding_matches_the_generic_encoder() {
        let path = temp_file("encoding");
        let mut state = RssState::new();
        state.feeds.push(RssFeed {
            url: "https://example.com/rss".to_string(),
            title: "Tïtle".to_string(),
            items: Vec::new(),
            last_poll: u64::MAX,
            poll_interval_secs: 900,
        });
        state.rules.push(RssRule {
            name: "r".to_string(),
            feed_url: String::new(),
            pattern: "*x*".to_string(),
        });
        state.seen_guids = vec!["a".to_string(), "bb".to_string()];
        save_rss_state(&path, &state).unwrap();
        let text = |value: &str| Value::Bytes(value.as_bytes().to_vec());
        let expected = bencode::encode(&Value::Dict(vec![
            (
                b"feeds".to_vec(),
                Value::List(vec![Value::Dict(vec![
                    (b"url".to_vec(), text("https://example.com/rss")),
                    (b"title".to_vec(), text("Tïtle")),
                    (b"last_poll".to_vec(), Value::Int(i64::MAX)),
                    (b"poll_interval".to_vec(), Value::Int(900)),
                ])]),
            ),
            (
                b"rules".to_vec(),
                Value::List(vec![Value::Dict(vec![
                    (b"name".to_vec(), text("r")),
                    (b"feed_url".to_vec(), text("")),
                    (b"pattern".to_vec(), text("*x*")),
                ])]),
            ),
            (b"seen".to_vec(), Value::List(vec![text("a"), text("bb")])),
        ]));
        assert_eq!(fs::read(&path).unwrap(), expected);
        let loaded = load_rss_state(&path).unwrap();
        assert_eq!(loaded.feeds[0].title, "Tïtle");
        assert_eq!(loaded.seen_guids, ["a", "bb"]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn seen_key_format_is_stable() {
        // Persisted in rss state; the encoding must never change.
        assert_eq!(
            seen_key("a", "b"),
            "v3:bfd8d48c5ab028a9c554ccc9d65ed6fc15a987f4f98cdd8bbfd5f17860493bac"
        );
    }

    #[test]
    fn failed_downloads_remain_eligible_until_success_is_recorded() {
        let item = FeedItem {
            title: "Release".to_string(),
            link: "https://example.com/release.torrent".to_string(),
            is_torrent: true,
            guid: "release-1".to_string(),
        };
        let rule = RssRule {
            name: "all".to_string(),
            feed_url: String::new(),
            pattern: "*".to_string(),
        };
        let feed_url = "https://example.com/feed";
        let mut seen = Vec::new();

        assert_eq!(
            match_rules(
                std::slice::from_ref(&item),
                std::slice::from_ref(&rule),
                &seen,
                feed_url,
            )
            .len(),
            1
        );
        // A failed enqueue/download does not call `remember_seen`, so the next poll retries it.
        assert_eq!(
            match_rules(
                std::slice::from_ref(&item),
                std::slice::from_ref(&rule),
                &seen,
                feed_url,
            )
            .len(),
            1
        );
        remember_seen(&mut seen, seen_key(feed_url, &item.guid));
        assert!(match_rules(
            std::slice::from_ref(&item),
            std::slice::from_ref(&rule),
            &seen,
            feed_url,
        )
        .is_empty());
    }

    #[test]
    fn rss_state_save_load_roundtrip() {
        let path = temp_file("state");
        let mut state = RssState::new();
        state.feeds.push(RssFeed {
            url: "http://example.com/rss".to_string(),
            title: "Test".to_string(),
            items: Vec::new(),
            last_poll: 12345,
            poll_interval_secs: 900,
        });
        state.rules.push(RssRule {
            name: "linux".to_string(),
            feed_url: String::new(),
            pattern: "*ubuntu*".to_string(),
        });
        state.seen_guids.push("guid-001".to_string());

        save_rss_state(&path, &state).unwrap();
        let loaded = load_rss_state(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(loaded.feeds.len(), 1);
        assert_eq!(loaded.feeds[0].url, "http://example.com/rss");
        assert_eq!(loaded.feeds[0].last_poll, 12345);
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].pattern, "*ubuntu*");
        assert_eq!(loaded.seen_guids, vec!["guid-001"]);
    }

    #[test]
    fn rss_state_roundtrips_at_configured_collection_limits() {
        let path = temp_file("collection-limits");
        let mut state = RssState::new();
        state.feeds = (0..MAX_RSS_FEEDS)
            .map(|index| RssFeed {
                url: format!("https://example.com/{index}"),
                title: "feed".to_string(),
                items: Vec::new(),
                last_poll: 0,
                poll_interval_secs: 900,
            })
            .collect();
        state.rules = (0..MAX_RSS_RULES)
            .map(|index| RssRule {
                name: format!("rule-{index}"),
                feed_url: String::new(),
                pattern: "*".to_string(),
            })
            .collect();

        save_rss_state(&path, &state).unwrap();
        let loaded = load_rss_state(&path).unwrap();
        assert_eq!(loaded.feeds.len(), MAX_RSS_FEEDS);
        assert_eq!(loaded.rules.len(), MAX_RSS_RULES);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(sidecar_path(&path, ".bak"));
    }

    #[test]
    fn rss_state_save_creates_parent_directory() {
        let path = temp_file("nested");
        let nested = path
            .parent()
            .unwrap()
            .join("rss-state-nested")
            .join("state.benc");
        let _ = fs::remove_dir_all(nested.parent().unwrap());
        let state = RssState::new();
        save_rss_state(&nested, &state).unwrap();
        assert!(nested.exists());
        let _ = fs::remove_file(&nested);
        let _ = fs::remove_dir_all(nested.parent().unwrap());
    }

    #[test]
    fn rss_state_recovers_from_backup() {
        let path = temp_file("backup");
        let mut state = RssState::new();
        state.feeds.push(RssFeed {
            url: "https://example.com/one".to_string(),
            title: "One".to_string(),
            items: Vec::new(),
            last_poll: 1,
            poll_interval_secs: 900,
        });
        save_rss_state(&path, &state).unwrap();
        state.feeds[0].title = "Two".to_string();
        save_rss_state(&path, &state).unwrap();
        fs::write(&path, b"corrupt").unwrap();

        let recovered = load_rss_state(&path).unwrap();
        assert_eq!(recovered.feeds[0].title, "One");

        fs::remove_file(&path).unwrap();
        let recovered_missing_primary = load_rss_state(&path).unwrap();
        assert_eq!(recovered_missing_primary.feeds[0].title, "One");

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(sidecar_path(&path, ".bak"));
    }

    #[cfg(unix)]
    #[test]
    fn rss_state_backup_rotation_replaces_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let path = temp_file("backup-symlink");
        let backup = sidecar_path(&path, ".bak");
        let outside = sidecar_path(&path, ".outside");
        let mut state = RssState::new();
        state.seen_guids.push("first".to_string());
        save_rss_state(&path, &state).unwrap();
        let original = fs::read(&path).unwrap();

        fs::write(&outside, b"must not be overwritten").unwrap();
        symlink(&outside, &backup).unwrap();
        state.seen_guids.push("second".to_string());

        // The backup is published by rename, which replaces the link itself
        // rather than writing through it.
        save_rss_state(&path, &state).unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"must not be overwritten");
        assert!(!fs::symlink_metadata(&backup)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&backup).unwrap(), original);

        let _ = fs::remove_file(&backup);
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_file(&path);
    }
}
