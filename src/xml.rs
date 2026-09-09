//! The little XML `WebDAV` carries, read for what Xmip needs from it: the
//! members a multistatus lists, and the token a LOCK grants.
//!
//! Not an XML parser. Elements are found by local name whatever prefix the
//! server chose — `D:`, `d:`, none — and entities are the five XML has.
//! A server that nests a `response` inside a `response` is handled; one
//! that puts markup in a CDATA section is not, and no `WebDAV` server does.

/// What PROPFIND said about one member: its href, whether it is a
/// collection, and whether an active lock was reported on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub href: String,
    pub collection: bool,
    pub locked: bool,
}

/// The members of a multistatus body, one per `<D:response>`.
#[must_use]
pub fn members(xml: &str) -> Vec<Member> {
    let mut members = Vec::new();
    let mut from = 0;
    while let Some((content, after)) = element(xml, from, "response") {
        if let Some((href, _)) = element(content, 0, "href") {
            members.push(Member {
                href: unescape(href.trim()),
                collection: has(content, "collection"),
                locked: has(content, "activelock"),
            });
        }
        from = after;
    }
    members
}

/// The lock token a LOCK response carries: `<D:locktoken><D:href>`.
#[must_use]
pub fn lock_token(xml: &str) -> Option<String> {
    let (content, _) = element(xml, 0, "locktoken")?;
    let (href, _) = element(content, 0, "href")?;
    Some(unescape(href.trim()))
}

/// One tag as it opens or closes: its local name, whether it closes, and
/// whether it closes itself.
fn tag(xml: &str, from: usize) -> Option<(usize, usize, &str, bool, bool)> {
    let open = from + xml[from..].find('<')?;
    let close = open + xml[open..].find('>')?;
    let raw = &xml[open + 1..close];
    let local = raw
        .trim_start_matches('/')
        .split([' ', '/'])
        .next()
        .unwrap_or("")
        .rsplit(':')
        .next()
        .unwrap_or("");
    Some((open, close, local, raw.starts_with('/'), raw.ends_with('/')))
}

/// The content of the first `name` element at or after `from`, whatever
/// namespace prefix it carries, and the offset after its close tag.
fn element<'a>(xml: &'a str, from: usize, name: &str) -> Option<(&'a str, usize)> {
    let mut at = from;
    loop {
        let (_, close, local, closes, empty) = tag(xml, at)?;
        if local == name && !closes && !empty {
            let start = close + 1;
            let end = start + closing(&xml[start..], name)?;
            let after = end + xml[end..].find('>')? + 1;
            return Some((&xml[start..end], after));
        }
        at = close + 1;
    }
}

/// Where the close tag of `name` begins, nested same-named elements skipped.
fn closing(rest: &str, name: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut at = 0;
    loop {
        let (open, close, local, closes, empty) = tag(rest, at)?;
        if local == name {
            if closes {
                if depth == 0 {
                    return Some(open);
                }
                depth -= 1;
            } else if !empty {
                depth += 1;
            }
        }
        at = close + 1;
    }
}

/// Whether a `name` element opens at all, `<D:collection/>` included.
fn has(xml: &str, name: &str) -> bool {
    let mut at = 0;
    while let Some((_, close, local, closes, _)) = tag(xml, at) {
        if local == name && !closes {
            return true;
        }
        at = close + 1;
    }
    false
}

/// `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&apos;` back to their characters.
#[must_use]
pub fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The characters XML text cannot carry bare, escaped.
#[must_use]
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multistatus_yields_its_members_and_a_lock_its_token() {
        let xml = r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:">
            <D:response><D:href>/orders/</D:href><D:propstat><D:prop>
              <D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>
            <d:response><d:href>/orders/a&amp;b.edi</d:href><d:propstat><d:prop>
              <d:resourcetype/><d:lockdiscovery><d:activelock/></d:lockdiscovery>
              </d:prop></d:propstat></d:response>
            <response><href>/orders/2.edi</href><propstat><prop><resourcetype/>
              </prop></propstat></response></D:multistatus>"#;
        let members = super::members(xml);
        assert_eq!(members.len(), 3);
        assert!(members[0].collection);
        assert_eq!(members[1].href, "/orders/a&b.edi");
        assert!(!members[1].collection);
        assert!(members[1].locked);
        assert!(!members[2].locked);
        assert_eq!(members[2].href, "/orders/2.edi");
        let lock = r#"<D:prop xmlns:D="DAV:"><D:lockdiscovery><D:activelock>
            <D:locktoken><D:href>opaquelocktoken:7</D:href></D:locktoken>
            </D:activelock></D:lockdiscovery></D:prop>"#;
        assert_eq!(lock_token(lock).as_deref(), Some("opaquelocktoken:7"));
        assert!(lock_token("<D:prop/>").is_none());
        assert!(
            super::members("<D:multistatus><D:response>").is_empty(),
            "unclosed"
        );
        assert_eq!(escape("a<b&\"c\""), "a&lt;b&amp;&quot;c&quot;");
    }
}
