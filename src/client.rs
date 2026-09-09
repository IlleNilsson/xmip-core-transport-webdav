//! The client's side: one connection to a `WebDAV` server, one method at a
//! time, each a request answered before the next is written.

use std::io::BufReader;
use std::net::TcpStream;
use std::time::Duration;

use http::target::HttpTarget;
use transport::error::{Result, protocol_error};
use transport::socket;

use crate::wire::{self, Request, Response};
use crate::xml::{self, Member};

/// The body PROPFIND asks with: what a member is, and who holds it.
const PROPFIND: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<D:propfind xmlns:D=\"DAV:\"><D:prop><D:resourcetype/><D:lockdiscovery/></D:prop></D:propfind>";

/// The body LOCK asks with: exclusive, for writing, owned by this node.
const LOCK: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<D:lockinfo xmlns:D=\"DAV:\"><D:lockscope><D:exclusive/></D:lockscope>\
<D:locktype><D:write/></D:locktype><D:owner>xmip</D:owner></D:lockinfo>";

/// One connected client.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    authority: String,
}

impl Client {
    /// Connect to the server `target` names.
    ///
    /// # Errors
    /// Where the server could not be reached, or the target asks for
    /// https, which this transport does not yet speak (ADR-0033).
    pub fn connect(target: &HttpTarget<'_>, timeout: Option<Duration>) -> Result<Self> {
        if target.secure {
            return Err(protocol_error(
                "https was asked for and this transport speaks plain http",
            ));
        }
        let stream = socket::connect_tcp(&target.address(), timeout)?;
        let (reader, writer) = socket::split(stream)?;
        Ok(Self {
            reader,
            writer,
            authority: target.authority.to_string(),
        })
    }

    /// The host and port this client is connected to.
    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Send one request and read its response, whatever the status.
    ///
    /// # Errors
    /// Where the connection broke or the answer was not HTTP.
    pub fn exchange(&mut self, request: &Request) -> Result<Response> {
        wire::write_request(&mut self.writer, &self.authority, request)?;
        wire::read_response(&mut self.reader)
    }

    /// One request that must succeed.
    fn expect_ok(&mut self, request: &Request) -> Result<Response> {
        let response = self.exchange(request)?;
        response.judge()?;
        Ok(response)
    }

    /// The members of `collection`, PROPFIND Depth 1: the collection
    /// itself first, as servers list it, then what it holds.
    ///
    /// # Errors
    /// Where the collection is not there or the server refused.
    pub fn list(&mut self, collection: &str) -> Result<Vec<Member>> {
        self.propfind(collection, "1")
    }

    /// What PROPFIND Depth 0 says about `href`, or `None` when it is not
    /// there.
    ///
    /// # Errors
    /// Where the server refused for any reason other than 404.
    pub fn find(&mut self, href: &str) -> Result<Option<Member>> {
        let request = Request::new("PROPFIND", href)
            .with_header("Depth", "0")
            .with_header("Content-Type", "application/xml")
            .with_body(PROPFIND);
        let response = self.exchange(&request)?;
        if response.status == 404 {
            return Ok(None);
        }
        response.judge()?;
        let text = String::from_utf8_lossy(&response.body).into_owned();
        Ok(xml::members(&text).into_iter().next().map(relative))
    }

    fn propfind(&mut self, href: &str, depth: &str) -> Result<Vec<Member>> {
        let request = Request::new("PROPFIND", href)
            .with_header("Depth", depth)
            .with_header("Content-Type", "application/xml")
            .with_body(PROPFIND);
        let response = self.expect_ok(&request)?;
        let text = String::from_utf8_lossy(&response.body).into_owned();
        Ok(xml::members(&text).into_iter().map(relative).collect())
    }

    /// The bytes at `href`.
    ///
    /// # Errors
    /// Where it is not there or the server refused.
    pub fn get(&mut self, href: &str) -> Result<Vec<u8>> {
        Ok(self.expect_ok(&Request::new("GET", href))?.body)
    }

    /// Store `bytes` at `href`.
    ///
    /// # Errors
    /// Where the collection is not there, somebody holds a lock on it, or
    /// the server refused.
    pub fn put(&mut self, href: &str, bytes: &[u8]) -> Result<()> {
        let request = Request::new("PUT", href)
            .with_header("Content-Type", "application/octet-stream")
            .with_body(bytes);
        self.expect_ok(&request).map(|_| ())
    }

    /// Remove `href`.
    ///
    /// # Errors
    /// Where it is not there, somebody holds a lock on it, or the server
    /// refused.
    pub fn delete(&mut self, href: &str) -> Result<()> {
        self.expect_ok(&Request::new("DELETE", href)).map(|_| ())
    }

    /// Remove `href` while holding its lock, `token` presented in `If`.
    ///
    /// # Errors
    /// As [`Self::delete`].
    pub fn delete_held(&mut self, href: &str, token: &str) -> Result<()> {
        let request = Request::new("DELETE", href).with_header("If", &format!("(<{token}>)"));
        self.expect_ok(&request).map(|_| ())
    }

    /// Create the collection `href`.
    ///
    /// # Errors
    /// Where it already exists or the server refused.
    pub fn mkcol(&mut self, href: &str) -> Result<()> {
        self.expect_ok(&Request::new("MKCOL", href)).map(|_| ())
    }

    /// Take an exclusive write lock on `href`; the token that names it.
    ///
    /// # Errors
    /// Where somebody else holds it — 423, retryable — or the server
    /// refused, or answered without a token.
    pub fn lock(&mut self, href: &str) -> Result<String> {
        let request = Request::new("LOCK", href)
            .with_header("Timeout", "Second-600")
            .with_header("Content-Type", "application/xml")
            .with_body(LOCK);
        let response = self.expect_ok(&request)?;
        let text = String::from_utf8_lossy(&response.body).into_owned();
        xml::lock_token(&text)
            .or_else(|| {
                response
                    .header("lock-token")
                    .map(|value| value.trim_matches(['<', '>']).to_string())
            })
            .ok_or_else(|| protocol_error("a LOCK answered without a lock token"))
    }

    /// Give the lock `token` on `href` back.
    ///
    /// # Errors
    /// Where the server refused: the token is not the lock's, or there is
    /// no lock.
    pub fn unlock(&mut self, href: &str, token: &str) -> Result<()> {
        let request = Request::new("UNLOCK", href).with_header("Lock-Token", &format!("<{token}>"));
        self.expect_ok(&request).map(|_| ())
    }
}

/// A member's href as a path, where the server wrote it as a full URL.
fn relative(mut member: Member) -> Member {
    if let Some(rest) = member
        .href
        .strip_prefix("http://")
        .or_else(|| member.href.strip_prefix("https://"))
    {
        member.href = rest.find('/').map_or("/", |at| &rest[at..]).to_string();
    }
    member
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_url_href_becomes_a_path() {
        let member = Member {
            href: "http://dav.example:8080/orders/1.edi".to_string(),
            collection: false,
            locked: false,
        };
        assert_eq!(relative(member).href, "/orders/1.edi");
        let bare = Member {
            href: "https://dav.example".to_string(),
            collection: true,
            locked: false,
        };
        assert_eq!(relative(bare).href, "/");
    }

    #[test]
    fn https_is_refused_rather_than_sent_in_the_clear() {
        let target = HttpTarget::parse("https://127.0.0.1:1/x").expect("parsed");
        let error = Client::connect(&target, None).err().expect("refused");
        assert!(!error.retryable);
        assert!(error.message.contains("https"));
    }
}
