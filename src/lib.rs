#![forbid(unsafe_code)]

//! Streams that arrive as files in a `WebDAV` collection. One file is one
//! Stream, its URL kept beside it.
//!
//! `WebDAV` is the drop box that sits behind a web server: a collection is a
//! directory, a member is a file, and every operation is an HTTP method —
//! PROPFIND lists, GET reads, PUT writes, DELETE removes, MKCOL creates.
//! A Receive Location lists the collection and takes each member; a Send
//! Location PUTs to a URL. Either may instead accept clients directly
//! through [`Session`], one client's worth of server over a [`Store`].
//!
//! **LOCK is the native claim**, ADR-0024. A `WebDAV` server grants one
//! exclusive write lock per resource and refuses the second with 423, so a
//! member locked here is claimed at the endpoint for every node at once;
//! [`WebDavTransport`] implements [`ResourceClaim`] with the lock token as
//! the claim token, and its receive locks each member before it takes it,
//! passing over what another node already holds.
//!
//! HTTP/1.1 with Content-Length, the connection kept between methods,
//! written and read by the estate's one HTTP/1.1 codec (`net::http`) —
//! this crate carried its own until 2026-09-24 — the target read by
//! `net::Endpoint`, and guarded where the scheme says so: TLS is
//! `xmip-core-library-tls`'s, per ADR-0033, reached through the http
//! technology's endpoint.
//!
//! A send target is `webdav://host:port/path/name` or `http://…`, or a
//! name alone inside this transport's collection. The origin URI is the
//! member's URL under this crate's scheme: `webdav://host:port/path/name`.

pub mod client;
pub mod session;
pub mod xml;

use std::net::TcpListener;
use std::time::Duration;

pub use client::Client;
use net::Endpoint;
pub use session::{Event, Session, Store};
use transport::error::{Result, protocol_error};
use transport::listening::Listening;
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
use transport::socket;
use transport::{Arrived, Artefact, Claimed, Directions, ResourceClaim, Transport};
pub use xml::Member;

/// The one member the loopback pair puts into the root collection.
const LOOPBACK_MEMBER: &str = "probe.bin";

#[derive(Clone)]
pub struct WebDavTransport {
    collection: String,
    timeout: Option<Duration>,
}

impl WebDavTransport {
    /// Speak to the collection at `collection_url` —
    /// `webdav://host:port/orders` or `http://host:port/orders`.
    #[must_use]
    pub fn new(collection_url: impl Into<String>) -> Self {
        Self {
            collection: as_http(&collection_url.into()),
            timeout: None,
        }
    }

    /// Give up on a server that stops mid-answer.
    #[must_use]
    pub const fn timing_out_after(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Connect to the collection's server.
    ///
    /// # Errors
    /// Where the collection URL cannot be read or the server not reached.
    pub fn connect(&self) -> Result<Client> {
        Client::connect(&Endpoint::parse(&self.collection)?, self.timeout)
    }

    /// Bind as the far end clients connect to, and report the address.
    /// The collection URL's authority is the bind address here.
    ///
    /// # Errors
    /// Where the address is taken, malformed, or not permitted.
    pub fn bind(&self) -> Result<(TcpListener, String)> {
        socket::bind_tcp(&Endpoint::parse(&self.collection)?.address())
    }

    /// Accept one client on an already-bound listener.
    ///
    /// # Errors
    /// Where the connection could not be accepted.
    pub fn accept_one(&self, listener: &TcpListener) -> Result<Session> {
        Session::accept(listener, self.timeout)
    }

    /// `target` as a full URL: as it is when it carries a scheme, else a
    /// name inside this transport's collection.
    fn resolve(&self, target: &str) -> String {
        if target.contains("://") {
            as_http(target)
        } else {
            format!(
                "{}/{}",
                self.collection.trim_end_matches('/'),
                target.trim_start_matches('/')
            )
        }
    }

    /// Connect to wherever `url` points, and the path there.
    fn open(&self, url: &str) -> Result<(Client, String)> {
        let endpoint = Endpoint::parse(url)?;
        let client = Client::connect(&endpoint, self.timeout)?;
        Ok((client, endpoint.path().to_string()))
    }

    /// Lock, take and remove one member, or `None` where another node
    /// holds it. The delete takes the lock with it (RFC 4918 section 9.6),
    /// so the lock is given back only where the take did not get that far.
    fn take(client: &mut Client, href: &str) -> Result<Option<Vec<u8>>> {
        let token = match client.lock(href) {
            Ok(token) => token,
            Err(error) if error.retryable => return Ok(None),
            Err(error) => return Err(error),
        };
        let outcome = client
            .get(href)
            .and_then(|bytes| client.delete_held(href, &token).map(|()| bytes));
        match outcome {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) => {
                let _ = client.unlock(href, &token);
                Err(error)
            }
        }
    }
}

/// `webdav://` is `http://` on the wire, and `webdavs://` is `https://`.
fn as_http(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("webdav://") {
        format!("http://{rest}")
    } else if let Some(rest) = url.strip_prefix("webdavs://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

impl Transport for WebDavTransport {
    fn name(&self) -> &'static str {
        "webdav"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    /// Every member of the collection that is a file and not held by
    /// somebody else: locked, fetched and deleted, in that order.
    fn receive(&self) -> Result<Vec<Arrived>> {
        let (mut client, path) = self.open(&self.collection)?;
        let mut arrived = Vec::new();
        for member in client.list(&path)? {
            if member.collection || member.href.trim_end_matches('/') == path.trim_end_matches('/')
            {
                continue;
            }
            if let Some(bytes) = Self::take(&mut client, &member.href)? {
                let origin = format!("webdav://{}{}", client.authority(), member.href);
                arrived.push(Arrived::new(origin, bytes));
            }
        }
        Ok(arrived)
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let url = self.resolve(target);
        if url.ends_with('/') {
            return Err(protocol_error(format!(
                "a send target that names a collection rather than a member: {url}"
            )));
        }
        let (mut client, path) = self.open(&url)?;
        client.put(&path, bytes)
    }

    fn claims(&self) -> Option<&dyn ResourceClaim> {
        Some(self)
    }
}

impl ResourceClaim for WebDavTransport {
    /// Whether the member is there and nobody reports a lock on it.
    fn is_available(&self, artefact: &Artefact) -> Result<bool> {
        let (mut client, path) = self.open(&self.resolve(artefact.address()))?;
        Ok(client
            .find(&path)?
            .is_some_and(|member| !member.locked && !member.collection))
    }

    /// LOCK it; the lock token is the claim token.
    fn claim(&self, artefact: &Artefact) -> Result<Claimed> {
        let (mut client, path) = self.open(&self.resolve(artefact.address()))?;
        let token = client.lock(&path)?;
        Ok(Claimed::new(artefact.clone(), token))
    }

    /// UNLOCK it with the token the claim carried.
    fn release(&self, claimed: Claimed) -> Result<()> {
        let (mut client, path) = self.open(&self.resolve(claimed.artefact.address()))?;
        client.unlock(&path, &claimed.token)
    }
}

impl WebDavTransport {
    /// Both ends on this machine: an ephemeral local port serving the root
    /// collection, the loopback timeout.
    #[must_use]
    pub fn loopback() -> Self {
        Self::new("webdav://127.0.0.1:0").timing_out_after(LOOPBACK_TIMEOUT)
    }
}

impl Loopback for WebDavTransport {
    /// A bound listener waiting for its one client. `WebDAV` keeps its
    /// connection, so the session reads it until the PUT.
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        let transport = self.clone();
        Ok(Box::new(Listening::new(
            move |listener: &TcpListener| {
                transport
                    .accept_one(listener)?
                    .next_put()?
                    .ok_or_else(|| protocol_error("the client closed without storing"))
            },
            self.bind()?,
        )))
    }

    /// PUT the payload as one member of the root collection, from a fresh
    /// near end on one connection to `address`.
    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        let near = Self {
            collection: format!("http://{address}"),
            ..self.clone()
        };
        near.send(LOOPBACK_MEMBER, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn far_end() -> (WebDavTransport, TcpListener, String) {
        let far_end = WebDavTransport::new("webdav://127.0.0.1:0/orders").timing_out_after(secs(2));
        let (listener, address) = far_end.bind().expect("binding");
        (far_end, listener, address)
    }

    #[test]
    fn a_send_puts_into_a_collection_and_a_receive_takes_it_back_out() {
        let (far_end, listener, address) = far_end();
        let sender = std::thread::spawn(move || {
            let near = WebDavTransport::new(format!("webdav://{address}/orders"))
                .timing_out_after(secs(2));
            near.send("1.edi", b"UNA:+.? '")?;
            near.send(&format!("http://{address}/orders/2.edi"), b"")?;
            assert!(near.send("orders/", b"x").is_err(), "a collection");
            let mut arrived = near.receive()?;
            arrived.sort_by(|a, b| a.origin_uri.cmp(&b.origin_uri));
            Ok::<_, transport::TransportError>(arrived)
        });
        let mut session = far_end.accept_one(&listener).expect("accepting");
        let mut store = Store::default();
        store.files.insert("/orders".to_string(), BTreeMap::new());
        session = session.with_store(store);
        let first = session.next_put().expect("first").expect("one");
        assert_eq!(first.bytes, b"UNA:+.? '");
        assert!(first.origin_uri.ends_with("/orders/1.edi"));
        assert!(session.next_put().expect("closed").is_none());
        let mut session = far_end
            .accept_one(&listener)
            .expect("second")
            .with_store(session.into_store());
        let second = session.next_put().expect("second").expect("one");
        assert!(second.bytes.is_empty());
        assert!(session.next_put().expect("closed").is_none());
        let store = session.into_store();
        assert_eq!(store.files_in("/orders").expect("collection").len(), 2);
        let mut session = far_end
            .accept_one(&listener)
            .expect("third")
            .with_store(store);
        let events = session.serve().expect("serving");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Locked(..)))
                .count(),
            2
        );
        assert!(
            events
                .iter()
                .any(|e| *e == Event::Deleted("/orders/2.edi".to_string()))
        );
        assert!(
            !events.iter().any(|e| matches!(e, Event::Unlocked(_))),
            "the delete took each lock with it, RFC 4918: {events:?}"
        );
        let arrived = sender.join().expect("thread").expect("round trip");
        assert_eq!(arrived.len(), 2);
        assert_eq!(arrived[0].bytes, b"UNA:+.? '");
        assert!(arrived[0].origin_uri.starts_with("webdav://127.0.0.1:"));
        assert!(arrived[0].origin_uri.ends_with("/orders/1.edi"));
        assert!(arrived[1].origin_uri.ends_with("/orders/2.edi"));
        assert!(session.store().files_in("/orders").expect("c").is_empty());
        assert!(session.store().locks.is_empty(), "every lock given back");
    }

    #[test]
    fn the_lock_is_the_claim_and_a_second_claim_is_refused() {
        let (far_end, listener, address) = far_end();
        let claimant = std::thread::spawn(move || {
            let near = WebDavTransport::new(format!("webdav://{address}/orders"))
                .timing_out_after(secs(2));
            let artefact = Artefact::new(format!("webdav://{address}/orders/1.edi"));
            assert!(near.is_available(&artefact)?);
            let claimed = near.claim(&artefact)?;
            assert!(claimed.token.starts_with("opaquelocktoken:"));
            let second = near.claim(&artefact).expect_err("held");
            assert!(second.retryable, "somebody holds it now and will let go");
            assert!(!near.is_available(&artefact)?);
            let mut client = near.connect()?;
            assert!(client.delete("/orders/1.edi").is_err(), "locked");
            assert!(client.unlock("/orders/1.edi", "wrong").is_err());
            drop(client);
            near.release(claimed)?;
            assert!(near.is_available(&artefact)?);
            assert!(!near.is_available(&Artefact::new("missing.edi"))?);
            Ok::<_, transport::TransportError>(())
        });
        let mut store = Store::default();
        store.insert("/orders", "1.edi", b"held");
        for _ in 0..8 {
            let mut session = far_end
                .accept_one(&listener)
                .expect("accepting")
                .with_store(store);
            session.serve().expect("serving");
            store = session.into_store();
        }
        claimant.join().expect("thread").expect("claiming");
        assert!(store.locks.is_empty());
        assert!(far_end.claims().is_some());
        assert_eq!(far_end.name(), "webdav");
    }

    #[test]
    fn a_server_that_does_not_speak_http_is_a_permanent_error() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address").to_string();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut writer = stream;
            net::head::read_head(&mut reader).expect("the request head");
            std::io::Write::write_all(&mut writer, b"220 mail.example ESMTP\r\n\r\n")
                .expect("write");
            // Read to the end rather than closing: a reset can discard bytes
            // the peer has not read yet, and then the failure under test is a
            // dropped connection rather than the answer that is not HTTP.
            let _ = std::io::Read::read_to_end(&mut reader, &mut Vec::new());
        });
        let error = WebDavTransport::new(format!("http://{address}/orders"))
            .timing_out_after(secs(2))
            .receive()
            .expect_err("refused");
        assert!(!error.retryable, "{error}");
        let (far_end, listener, address) = far_end();
        std::thread::spawn(move || {
            let mut session = far_end.accept_one(&listener).expect("accepting");
            let _ = session.serve();
        });
        let near = WebDavTransport::new(format!("webdav://{address}")).timing_out_after(secs(2));
        let mut client = near.connect().expect("connecting");
        client.mkcol("/new").expect("created");
        assert!(client.mkcol("/new").is_err(), "twice");
        assert!(client.get("/new/none").is_err(), "not there");
        assert!(client.lock("/new/none").is_err(), "nothing to lock");
        client.put("/new/a.txt", b"a").expect("stored");
        let members = client.list("/new").expect("listed");
        assert_eq!(members.len(), 2);
        assert!(members[0].collection);
        assert_eq!(members[1].href, "/new/a.txt");
    }

    #[test]
    fn the_loopback_puts_one_member_through_its_own_session() {
        let pair = WebDavTransport::loopback();
        let arrived = pair.round(b"a member").expect("round");
        assert_eq!(arrived.bytes, b"a member");
        assert!(arrived.origin_uri.starts_with("webdav://127.0.0.1:"));
        assert!(arrived.origin_uri.ends_with("/probe.bin"));
        assert_eq!(pair.name(), "webdav");
        assert_eq!(pair.ceiling(), None);
    }

    /// The Playground's edge payloads, written here so the crate does not
    /// depend on it.
    fn edge_payloads() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("empty", Vec::new()),
            ("one byte", vec![0x2a]),
            ("every byte", (0..=255).collect()),
            ("nul run", vec![0; 512]),
            ("high bytes", vec![0xff; 512]),
            ("crlf storm", b"\r\n".repeat(400)),
        ]
    }

    #[test]
    fn the_loopback_returns_the_edge_payloads_whole() {
        let pair = WebDavTransport::loopback();
        for (name, payload) in edge_payloads() {
            assert!(pair.refuses(&payload).is_none(), "{name}");
            let arrived = pair.round(&payload).expect(name);
            assert_eq!(arrived.bytes, payload, "{name}");
        }
    }
}
