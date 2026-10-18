//! The little URL handling the bridge needs: origins, paths and query strings.

/// `https://host:port/path?query` -> (`https://host:port`, `/path?query`).
pub fn split(url: &str) -> Option<(&str, &str)> {
    let scheme_end = url.find("://")? + 3;
    let path_start = url[scheme_end..].find(['/', '?', '#']).map_or(url.len(), |i| scheme_end + i);
    (path_start > scheme_end).then(|| (&url[..path_start], &url[path_start..]))
}

pub fn origin(url: &str) -> Option<&str> {
    split(url).map(|(origin, _)| origin)
}

/// Path without query or fragment, never empty.
pub fn path(url: &str) -> &str {
    let rest = split(url).map_or("", |(_, rest)| rest);
    let path = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    if path.is_empty() { "/" } else { path }
}

pub fn host(url: &str) -> &str {
    origin(url).and_then(|o| o.split_once("://")).map_or(url, |(_, host)| host)
}

pub fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn decode_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn query_string(params: &[(&str, &str)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", encode_component(k), encode_component(v))).collect::<Vec<_>>().join("&")
}

pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode_component(k), decode_component(v))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        assert_eq!(split("https://a.b:8/mcp?x=1"), Some(("https://a.b:8", "/mcp?x=1")));
        assert_eq!(split("http://h"), Some(("http://h", "")));
        assert_eq!(path("http://h"), "/");
        assert_eq!(path("http://h/a/b?c"), "/a/b");
        assert_eq!(host("http://127.0.0.1:3773/mcp"), "127.0.0.1:3773");
    }

    #[test]
    fn encodes_queries() {
        let q = query_string(&[("redirect_uri", "http://127.0.0.1:1/cb"), ("scope", "a b")]);
        assert_eq!(q, "redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcb&scope=a%20b");
        assert_eq!(
            parse_query("code=a%2Fb&state=x+y&bad=%zz&end=%4"),
            [
                ("code".into(), "a/b".into()),
                ("state".into(), "x y".into()),
                ("bad".into(), "%zz".into()),
                ("end".into(), "%4".into())
            ]
        );
    }
}
