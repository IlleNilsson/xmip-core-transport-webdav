//! The server's side of one connection: what a test puts at the far end,
//! and what the playground stands up in place of a `WebDAV` server.
//!
//! Not a server. One session serves one client over a [`Store`] kept in
//! memory — collections of files, and the locks held on them — answering
//! each method with the status a real server would. Users are not checked.
//! A Location talks to a real server through [`crate::Client`].

use std::collections::BTreeMap;
use std::io::BufReader;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use transport::Arrived;
use transport::error::Result;
use transport::socket;

use codec::xml::escape;
use net::http::{Request, Response};

/// What the session serves: collections of files, and the locks on them.
///
/// A collection is keyed by its path without the trailing slash — `/orders`
/// — and the root is the empty string. Locks are keyed by the full href.
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub files: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    pub locks: BTreeMap<String, String>,
    next_token: u64,
}

impl Store {
    /// Put `bytes` as `name` in `collection`, creating the collection.
    pub fn insert(&mut self, collection: &str, name: &str, bytes: &[u8]) {
        self.files
            .entry(collection.trim_end_matches('/').to_string())
            .or_default()
            .insert(name.to_string(), bytes.to_vec());
    }

    /// The files in `collection`, or none where it does not exist.
    #[must_use]
    pub fn files_in(&self, collection: &str) -> Option<&BTreeMap<String, Vec<u8>>> {
        self.files.get(collection.trim_end_matches('/'))
    }
}

/// What the client did, as [`Session::next_event`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The client listed this collection.
    Listed(String),
    /// The client stored a file; here is the Stream.
    Put(Arrived),
    /// The client fetched this href.
    Got(String),
    /// The client removed this href.
    Deleted(String),
    /// The client created this collection.
    Created(String),
    /// The client locked this href and holds this token.
    Locked(String, String),
    /// The client unlocked this href.
    Unlocked(String),
    /// The client asked something the store refused, with this status.
    Refused(String, u16),
}

pub struct Session {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    peer: SocketAddr,
    store: Store,
}

impl Session {
    /// Accept one client on `listener` over an empty store with a root
    /// collection.
    ///
    /// # Errors
    /// Where the connection could not be accepted.
    pub fn accept(listener: &TcpListener, timeout: Option<Duration>) -> Result<Self> {
        let (stream, peer) = socket::accept_tcp(listener, timeout)?;
        let (reader, writer) = socket::split(stream)?;
        let mut store = Store::default();
        store.files.entry(String::new()).or_default();
        Ok(Self {
            reader,
            writer,
            peer,
            store,
        })
    }

    /// Serve this store instead — the way one session hands its state to
    /// the next connection.
    #[must_use]
    pub fn with_store(mut self, store: Store) -> Self {
        self.store = store;
        self
    }

    /// What the store holds now.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The store, for the next session to carry on with.
    #[must_use]
    pub fn into_store(self) -> Store {
        self.store
    }

    /// The next file the client stores, or `None` when it closed.
    ///
    /// # Errors
    /// Where the connection broke, or nothing arrived before the timeout.
    pub fn next_put(&mut self) -> Result<Option<Arrived>> {
        loop {
            match self.next_event()? {
                Some(Event::Put(arrived)) => return Ok(Some(arrived)),
                Some(_) => {}
                None => return Ok(None),
            }
        }
    }

    /// Serve until the client closes.
    ///
    /// # Errors
    /// Where the connection broke, or nothing arrived before the timeout.
    pub fn serve(&mut self) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        while let Some(event) = self.next_event()? {
            events.push(event);
        }
        Ok(events)
    }

    /// The next thing the client did, or `None` when it closed.
    ///
    /// # Errors
    /// Where the connection broke, nothing arrived before the timeout, or
    /// what arrived was not HTTP.
    pub fn next_event(&mut self) -> Result<Option<Event>> {
        let Some(request) = net::http::read_request(&mut self.reader)? else {
            return Ok(None);
        };
        let (event, response) = self.answer(&request);
        let response = response.header("Connection", "keep-alive");
        net::http::write_response(&mut self.writer, &response)?;
        Ok(Some(event))
    }

    fn answer(&mut self, request: &Request) -> (Event, Response) {
        let href = request.path.trim_end_matches('/').to_string();
        let (collection, name) = href.rsplit_once('/').unwrap_or(("", ""));
        let (collection, name) = (collection.to_string(), name.to_string());
        let refused = |status: u16| (Event::Refused(href.clone(), status), Response::new(status));
        match request.method.as_str() {
            "PROPFIND" => self.propfind(&href, request.header_value("Depth").unwrap_or("1")),
            "GET" => match self.file(&collection, &name) {
                Some(bytes) => (Event::Got(href.clone()), Response::new(200).body(&bytes)),
                None => refused(404),
            },
            "PUT" => {
                if !self.store.files.contains_key(&collection) {
                    return refused(409);
                }
                if !self.may_write(&href, request) {
                    return refused(423);
                }
                let existed = self.file(&collection, &name).is_some();
                self.store.insert(&collection, &name, &request.body);
                let origin = format!("webdav://{}{href}", self.peer);
                (
                    Event::Put(Arrived::new(origin, request.body.clone())),
                    Response::new(if existed { 204 } else { 201 }),
                )
            }
            "DELETE" => {
                if !self.may_write(&href, request) {
                    return refused(423);
                }
                let files = self.store.files.get_mut(&collection);
                if files.is_some_and(|files| files.remove(&name).is_some()) {
                    self.store.locks.remove(&href);
                    (Event::Deleted(href.clone()), Response::new(204))
                } else if self.store.files.remove(&href).is_some() {
                    (Event::Deleted(href.clone()), Response::new(204))
                } else {
                    refused(404)
                }
            }
            "MKCOL" => {
                if self.store.files.contains_key(&href) {
                    return refused(405);
                }
                self.store.files.insert(href.clone(), BTreeMap::new());
                (Event::Created(href.clone()), Response::new(201))
            }
            "LOCK" => self.lock(&href, &collection, &name),
            "UNLOCK" => {
                let presented = request
                    .header_value("Lock-Token")
                    .map(|value| value.trim_matches(['<', '>']))
                    .unwrap_or_default();
                match self.store.locks.get(&href) {
                    None => refused(409),
                    Some(held) if held != presented => refused(403),
                    Some(_) => {
                        self.store.locks.remove(&href);
                        (Event::Unlocked(href.clone()), Response::new(204))
                    }
                }
            }
            _ => refused(405),
        }
    }

    fn file(&self, collection: &str, name: &str) -> Option<Vec<u8>> {
        self.store.files.get(collection)?.get(name).cloned()
    }

    /// Whether `href` is unlocked, or the request carries its token in `If`.
    fn may_write(&self, href: &str, request: &Request) -> bool {
        self.store.locks.get(href).is_none_or(|token| {
            request
                .header_value("If")
                .is_some_and(|value| value.contains(&format!("<{token}>")))
        })
    }

    fn propfind(&self, href: &str, depth: &str) -> (Event, Response) {
        let mut entries = Vec::new();
        if let Some(files) = self.store.files.get(href) {
            entries.push(self.entry(&format!("{href}/"), true));
            if depth != "0" {
                for name in files.keys() {
                    entries.push(self.entry(&format!("{href}/{name}"), false));
                }
            }
        } else {
            let (collection, name) = href.rsplit_once('/').unwrap_or(("", ""));
            if self.file(collection, name).is_none() {
                return (Event::Refused(href.to_string(), 404), Response::new(404));
            }
            entries.push(self.entry(href, false));
        }
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <D:multistatus xmlns:D=\"DAV:\">{}</D:multistatus>",
            entries.concat()
        );
        (
            Event::Listed(href.to_string()),
            Response::new(207)
                .header("Content-Type", "application/xml; charset=utf-8")
                .body(body.as_bytes()),
        )
    }

    fn entry(&self, href: &str, collection: bool) -> String {
        let kind = if collection {
            "<D:resourcetype><D:collection/></D:resourcetype>"
        } else {
            "<D:resourcetype/>"
        };
        let lock = self
            .store
            .locks
            .get(href.trim_end_matches('/'))
            .map(|token| format!("<D:lockdiscovery>{}</D:lockdiscovery>", active_lock(token)))
            .unwrap_or_default();
        format!(
            "<D:response><D:href>{}</D:href><D:propstat><D:prop>{kind}{lock}</D:prop>\
             <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
            escape(href)
        )
    }

    fn lock(&mut self, href: &str, collection: &str, name: &str) -> (Event, Response) {
        if self.file(collection, name).is_none() {
            return (Event::Refused(href.to_string(), 404), Response::new(404));
        }
        if self.store.locks.contains_key(href) {
            return (Event::Refused(href.to_string(), 423), Response::new(423));
        }
        self.store.next_token += 1;
        let token = format!("opaquelocktoken:xmip-{}", self.store.next_token);
        self.store.locks.insert(href.to_string(), token.clone());
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:prop xmlns:D=\"DAV:\">\
             <D:lockdiscovery>{}</D:lockdiscovery></D:prop>",
            active_lock(&token)
        );
        (
            Event::Locked(href.to_string(), token.clone()),
            Response::new(200)
                .header("Lock-Token", &format!("<{token}>"))
                .header("Content-Type", "application/xml; charset=utf-8")
                .body(body.as_bytes()),
        )
    }
}

fn active_lock(token: &str) -> String {
    format!(
        "<D:activelock><D:locktype><D:write/></D:locktype>\
         <D:lockscope><D:exclusive/></D:lockscope><D:depth>0</D:depth>\
         <D:timeout>Second-600</D:timeout>\
         <D:locktoken><D:href>{}</D:href></D:locktoken></D:activelock>",
        escape(token)
    )
}
