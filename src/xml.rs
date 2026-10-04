//! The little XML `WebDAV` carries, read for what Xmip needs from it: the
//! members a multistatus lists, and the token a LOCK grants.
//!
//! Read with the estate's flat scan (`codec::xml`): elements by local name
//! whatever prefix the server chose — `D:`, `d:`, none — a `response`
//! nested inside a `response` stepped over, entities read as every XML
//! reader in the estate reads them. Markup in a CDATA section is not
//! understood, and no `WebDAV` server puts it there.

use codec::xml::{content, elements, text};
use transport::error::Result;

/// What PROPFIND said about one member: its href, whether it is a
/// collection, whether an active lock was reported on it, and its stamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub href: String,
    pub collection: bool,
    pub locked: bool,
    /// What changes whenever the member is written again: its `getetag`,
    /// or else its `getlastmodified` with its `getcontentlength` (RFC 4918
    /// section 15); `None` where the server reported neither.
    pub stamp: Option<String>,
}

/// The members of a multistatus body, one per `<D:response>`.
///
/// # Errors
///
/// An href holds an entity XML does not define.
pub fn members(xml: &str) -> Result<Vec<Member>> {
    let mut members = Vec::new();
    for response in elements(xml, "response") {
        let response = response.content();
        if let Some(href) = content(response, "href") {
            members.push(Member {
                href: codec::xml::unescape(href.trim())?,
                collection: has(response, "collection"),
                locked: has(response, "activelock"),
                stamp: stamp(response)?,
            });
        }
    }
    Ok(members)
}

/// A member's `getetag`, or else its `getlastmodified` and
/// `getcontentlength` together, or `None` where it reported none of them.
fn stamp(response: &str) -> Result<Option<String>> {
    let read = |name| -> Result<Option<String>> {
        Ok(text(response, name)?
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()))
    };
    if let Some(tag) = read("getetag")? {
        return Ok(Some(tag));
    }
    Ok(
        match (read("getlastmodified")?, read("getcontentlength")?) {
            (None, None) => None,
            (modified, length) => Some(format!(
                "{} {}",
                modified.unwrap_or_default(),
                length.unwrap_or_default()
            )),
        },
    )
}

/// The lock token a LOCK response carries: `<D:locktoken><D:href>`.
///
/// # Errors
///
/// The token holds an entity XML does not define.
pub fn lock_token(xml: &str) -> Result<Option<String>> {
    let Some(href) = content(xml, "locktoken").and_then(|token| content(token, "href")) else {
        return Ok(None);
    };
    Ok(Some(codec::xml::unescape(href.trim())?))
}

/// Whether a `name` element is there at all, `<D:collection/>` included.
fn has(xml: &str, name: &str) -> bool {
    elements(xml, name).next().is_some()
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
              <d:getetag>"7"</d:getetag><d:getcontentlength>3</d:getcontentlength>
              </d:prop></d:propstat></d:response>
            <response><href>/orders/2.edi</href><propstat><prop><resourcetype/>
              <getlastmodified>Sun, 04 Oct 2026 08:00:00 GMT</getlastmodified>
              <getcontentlength>12</getcontentlength>
              </prop></propstat></response></D:multistatus>"#;
        let members = super::members(xml).expect("read");
        assert_eq!(members.len(), 3);
        assert!(members[0].collection);
        assert_eq!(members[1].href, "/orders/a&b.edi");
        assert!(!members[1].collection);
        assert!(members[1].locked);
        assert!(!members[2].locked);
        assert_eq!(members[2].href, "/orders/2.edi");
        assert_eq!(members[0].stamp, None);
        assert_eq!(members[1].stamp.as_deref(), Some("\"7\""), "the ETag first");
        assert_eq!(
            members[2].stamp.as_deref(),
            Some("Sun, 04 Oct 2026 08:00:00 GMT 12"),
            "else when it was written, and how long it is"
        );
        let lock = r#"<D:prop xmlns:D="DAV:"><D:lockdiscovery><D:activelock>
            <D:locktoken><D:href>opaquelocktoken:7</D:href></D:locktoken>
            </D:activelock></D:lockdiscovery></D:prop>"#;
        assert_eq!(
            lock_token(lock).expect("read").as_deref(),
            Some("opaquelocktoken:7")
        );
        assert!(lock_token("<D:prop/>").expect("read").is_none());
        assert!(
            super::members("<D:multistatus><D:response>")
                .expect("read")
                .is_empty(),
            "unclosed"
        );
    }
}
