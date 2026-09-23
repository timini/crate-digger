//! Public pages: unauthenticated GET only, robots.txt respected, size
//! limited by the transport, HTML reduced to text.
use std::net::IpAddr;

use cd_core::adapters::{AdapterError, AdapterResult};
use url::{Host, Url};

use crate::http::{Request, Transport};

pub const USER_AGENT_TOKEN: &str = "cratedigger";
const MAX_TEXT: usize = 100_000;

pub struct Page {
    pub url: String,
    pub text: String,
}

fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6.to_ipv4_mapped().is_some_and(|v4| private_ip(IpAddr::V4(v4)))
        }
    }
}

/// Only public http(s) pages: no credentials in the URL and no local or
/// private network hosts, so a page link cannot reach services on this machine.
pub fn public_url(value: &str) -> AdapterResult<Url> {
    let bad = |m: &str| AdapterError::Invalid(m.to_string());
    let mut url =
        Url::parse(value.trim()).map_err(|_| bad("Enter a full web address starting with https://."))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("Only http and https pages can be read."));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("Remove the user name or password from the address."));
    }
    let local = match url.host() {
        None => true,
        Some(Host::Ipv4(ip)) => private_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => private_ip(IpAddr::V6(ip)),
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".internal")
                || d.ends_with(".lan")
                || !d.contains('.')
        }
    };
    if local {
        return Err(bad(
            "Only public web pages can be read, not local network addresses.",
        ));
    }
    url.set_fragment(None);
    Ok(url)
}

/// Whether robots.txt lets this app fetch `path` (RFC 9309: the group for
/// our product token, else `*`; longest match wins; Allow wins ties).
pub fn robots_allows(robots: &str, path: &str) -> bool {
    // (user agents, rules as (allow, path pattern))
    type Group = (Vec<String>, Vec<(bool, String)>);
    let mut groups: Vec<Group> = vec![];
    let mut in_agents = false;
    for line in robots.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        let (field, value) = (field.trim().to_ascii_lowercase(), value.trim().to_string());
        match field.as_str() {
            "user-agent" => {
                if !in_agents {
                    groups.push((vec![], vec![]));
                }
                in_agents = true;
                if let Some(g) = groups.last_mut() {
                    g.0.push(value.to_ascii_lowercase());
                }
            }
            "allow" | "disallow" => {
                in_agents = false;
                if let Some(g) = groups.last_mut() {
                    g.1.push((field == "allow", value));
                }
            }
            _ => {}
        }
    }
    let rules: Vec<&(bool, String)> = {
        let ours: Vec<_> = groups
            .iter()
            .filter(|g| {
                g.0.iter()
                    .any(|a| a != "*" && USER_AGENT_TOKEN.contains(a.as_str()))
            })
            .flat_map(|g| &g.1)
            .collect();
        if ours.is_empty() {
            groups
                .iter()
                .filter(|g| g.0.iter().any(|a| a == "*"))
                .flat_map(|g| &g.1)
                .collect()
        } else {
            ours
        }
    };
    let mut best: Option<(usize, bool)> = None;
    for (allow, pattern) in rules {
        if pattern.is_empty() {
            continue;
        }
        if pattern_matches(pattern, path) {
            let len = pattern.len();
            match best {
                Some((l, a)) if l > len || (l == len && a) => {}
                _ => best = Some((len, *allow)),
            }
        }
    }
    best.is_none_or(|(_, allow)| allow)
}

fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if let Some(found) = path[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }
    !anchored || pos == path.len() || parts.last().is_some_and(|p| p.is_empty())
}

/// Checks robots.txt for this URL's site, then fetches it.
fn get_allowed(transport: &dyn Transport, url: &Url) -> AdapterResult<crate::http::Response> {
    let mut robots_url = url.clone();
    robots_url.set_path("/robots.txt");
    robots_url.set_query(None);
    let robots = transport.send(Request::get(robots_url.to_string()))?;
    let allowed = match robots.status {
        200..=299 => {
            let mut path = url.path().to_string();
            if let Some(q) = url.query() {
                path.push('?');
                path.push_str(q);
            }
            robots_allows(&robots.body, &path)
        }
        // RFC 9309: an unavailable robots.txt (4xx) means no restrictions.
        400..=499 => true,
        _ => return Err(AdapterError::Unavailable(
            "The site's robots.txt could not be read, so the page was not fetched. Paste the text instead."
                .into(),
        )),
    };
    if !allowed {
        return Err(AdapterError::Invalid(
            "This site's robots.txt does not allow automated reading of that page. Paste the text instead."
                .into(),
        ));
    }
    transport.send(Request::get(url.to_string()))
}

pub fn fetch(transport: &dyn Transport, value: &str) -> AdapterResult<Page> {
    let mut url = public_url(value)?;
    let mut page = get_allowed(transport, &url)?;
    // Follow a few redirects by hand so each hop is checked like the first.
    for _ in 0..3 {
        let Some(next) = page
            .location
            .as_deref()
            .filter(|_| (300..400).contains(&page.status))
        else {
            break;
        };
        let next = url
            .join(next)
            .map_err(|_| AdapterError::Invalid("The page redirected to an invalid address.".into()))?;
        url = public_url(next.as_str())?;
        page = get_allowed(transport, &url)?;
    }
    let page = page.checked()?;
    let text = html_to_text(&page.body);
    if text.trim().is_empty() {
        return Err(AdapterError::Invalid(
            "The page has no readable text. It may need JavaScript; paste the text instead.".into(),
        ));
    }
    Ok(Page {
        url: url.to_string(),
        text,
    })
}

fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .to_ascii_lowercase()
        .find(needle)
        .map(|i| i + from)
}

/// Reduces HTML to text lines. Plain text passes through unchanged.
pub fn html_to_text(html: &str) -> String {
    if !html.contains('<') {
        return html.chars().take(MAX_TEXT).collect();
    }
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript", "template", "svg"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}");
        while let Some(start) = find_ci(&s, &open, 0) {
            let end = find_ci(&s, &close, start)
                .and_then(|c| s[c..].find('>').map(|g| c + g + 1))
                .unwrap_or(s.len());
            s.replace_range(start..end, "\n");
        }
    }
    while let Some(start) = s.find("<!--") {
        let end = s[start..].find("-->").map(|e| start + e + 3).unwrap_or(s.len());
        s.replace_range(start..end, "");
    }
    let mut out = String::with_capacity(s.len() / 2);
    let mut rest = s.as_str();
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[lt + 1..lt + gt].trim_start_matches('/').to_ascii_lowercase();
        let name: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if matches!(
            name.as_str(),
            "br" | "p"
                | "div"
                | "li"
                | "tr"
                | "td"
                | "th"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "section"
                | "article"
                | "ul"
                | "ol"
                | "table"
                | "dt"
                | "dd"
                | "blockquote"
                | "pre"
        ) {
            out.push(if name == "td" || name == "th" { ' ' } else { '\n' });
        }
        rest = &rest[lt + gt + 1..];
    }
    out.push_str(rest);
    let decoded = decode_entities(&out);
    let mut text = String::new();
    for line in decoded.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() {
            text.push_str(&line);
            text.push('\n');
        }
        if text.len() > MAX_TEXT {
            break;
        }
    }
    text
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let end = tail.find(';').filter(|e| *e <= 10);
        let decoded = end.and_then(|e| {
            let name = &tail[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                "ndash" => Some('–'),
                "mdash" => Some('—'),
                _ if name.starts_with("#x") || name.starts_with("#X") => {
                    u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32)
                }
                _ if name.starts_with('#') => name[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &tail[e + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
