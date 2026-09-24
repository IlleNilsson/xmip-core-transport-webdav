//! The client's side: one connection to a `WebDAV` server, one method at a
//! time, each a request answered before the next is written. The requests
//! and answers are HTTP's, written and read by the http technology's codec
//! (`http::message`), the connection kept alive between them.

use std::io::BufReader;
use std::time::Duration;

use http::endpoint::{self, Connection};
use http::message::{self, Request, Response};
use http::target::HttpTarget;
use transport::error::{Result, protocol_error};

use crate::xml::{self, Member};

/// Who answers, as a refusal names it.
const SERVICE: &str = "the WebDAV server";
/// The one status beyond HTTP's own that is worth repeating: somebody holds
/// the resource now and will let go.
const LOCKED: &str = "Locked";

/// The body PROPFIND asks with: what a member is, and who holds it.
const PROPFIND: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<D:propfind xmlns:D=\"DAV:\"><D:prop><D:resourcetype/><D:lockdiscovery/></D:prop></D:propfind>";

/// The body LOCK asks with: exclusive, for writing, owned by this node.
const LOCK: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<D:lockinfo xmlns:D=\"DAV:\"><D:lockscope><D:exclusive/></D:lockscope>\
<D:locktype><D:write/></D:locktype><D:owner>xmip</D:owner></D:lockinfo>";

/// One connected client.
pub struct Client {
    // One connection, read through a buffer and written through the same
    // object: a guarded connection cannot be split into two halves the way a
    // socket can.
    stream: BufReader<Box<dyn Connection>>,
    authority: String,
}

impl Client {
    /// Connect to the server `target` names.
    ///
    /// `webdavs://` and `https://` are guarded, through the same endpoint
    /// every technology riding on HTTP connects by, and so by the estate's
    /// one TLS (ADR-0033). Until 2026-09-23 this refused them.
    ///
    /// # Errors
    /// Where the server could not be reached, or the target asks for TLS
    /// this build has no `tls` feature for.
    pub fn connect(target: &HttpTarget<'_>, timeout: Option<Duration>) -> Result<Self> {
        let scheme = if target.secure { "https" } else { "http" };
        let endpoint = format!("{scheme}://{}{}", target.authority, target.path);

        Ok(Self {
            stream: BufReader::new(endpoint::connect(&endpoint, timeout)?),
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
        let request = request
            .clone()
            .header("Host", &self.authority)
            .header("Connection", "keep-alive");
        message::write_request(self.stream.get_mut(), &request)?;
        message::read_response(&mut self.stream)
    }

    /// One request that must succeed.
    fn expect_ok(&mut self, request: &Request) -> Result<Response> {
        judged(self.exchange(request)?)
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
            .header("Depth", "0")
            .header("Content-Type", "application/xml")
            .body(PROPFIND);
        let response = self.exchange(&request)?;
        if response.status == 404 {
            return Ok(None);
        }
        let response = judged(response)?;
        let text = String::from_utf8_lossy(&response.body).into_owned();
        Ok(xml::members(&text)?.into_iter().next().map(relative))
    }

    fn propfind(&mut self, href: &str, depth: &str) -> Result<Vec<Member>> {
        let request = Request::new("PROPFIND", href)
            .header("Depth", depth)
            .header("Content-Type", "application/xml")
            .body(PROPFIND);
        let response = self.expect_ok(&request)?;
        let text = String::from_utf8_lossy(&response.body).into_owned();
        Ok(xml::members(&text)?.into_iter().map(relative).collect())
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
            .header("Content-Type", "application/octet-stream")
            .body(bytes);
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
        let request = Request::new("DELETE", href).header("If", &format!("(<{token}>)"));
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
            .header("Timeout", "Second-600")
            .header("Content-Type", "application/xml")
            .body(LOCK);
        let response = self.expect_ok(&request)?;
        let text = String::from_utf8_lossy(&response.body).into_owned();
        xml::lock_token(&text)?
            .or_else(|| {
                response
                    .header_value("lock-token")
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
        let request = Request::new("UNLOCK", href).header("Lock-Token", &format!("<{token}>"));
        self.expect_ok(&request).map(|_| ())
    }
}

/// `Ok` for a 2xx, else the failure with HTTP's judgement of it — and 423
/// Locked worth repeating too, which HTTP alone does not say.
fn judged(response: Response) -> Result<Response> {
    message::judge(
        SERVICE,
        response,
        |answer| message::reason(answer.status).to_string(),
        |code| code == LOCKED,
    )
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
    fn a_lock_is_worth_waiting_for_and_a_missing_member_is_not() {
        assert!(judged(Response::new(207)).is_ok());
        let locked = judged(Response::new(423)).expect_err("locked");
        assert!(locked.retryable);
        assert_eq!(locked.message, "the WebDAV server answered 423 Locked");
        assert!(judged(Response::new(503)).expect_err("server").retryable);
        assert!(!judged(Response::new(404)).expect_err("missing").retryable);
    }

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
    fn a_guarded_target_is_carried_to_the_endpoint_rather_than_refused() {
        // Nothing listens on port 1, so the connection fails where a TLS
        // build reaches: at the socket, not at a refusal of its own.
        let target = HttpTarget::parse("https://127.0.0.1:1/x").expect("parsed");
        let error = Client::connect(&target, None).err().expect("nothing there");

        assert!(
            !error.message.contains("speaks plain http"),
            "{}",
            error.message
        );
    }
}
