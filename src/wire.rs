//! HTTP/1.1 as `WebDAV` uses it: one request, one response, each a head and
//! a body whose length the head already said. The little XML that
//! PROPFIND and LOCK carry is read in `xml.rs`.
//!
//! Content-Length only. Nothing here chunks, and a response that arrives
//! chunked is refused rather than misread.

use std::io::{BufRead, Read, Write};

use transport::error::{Result, TransportError, classify, protocol_error};
use transport::wire::{MAX_BODY, header, read_head};

/// One request, either side of the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// `method` on `path`, no headers yet.
    #[must_use]
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// With this header too.
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// With this body.
    #[must_use]
    pub fn with_body(mut self, body: &[u8]) -> Self {
        self.body = body.to_vec();
        self
    }

    /// One header's value, however it was capitalised.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }
}

/// One response, either side of the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// `status`, no headers, no body.
    #[must_use]
    pub const fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// With this header too.
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// With this body.
    #[must_use]
    pub fn with_body(mut self, body: &[u8]) -> Self {
        self.body = body.to_vec();
        self
    }

    /// One header's value, however it was capitalised.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }

    /// `Ok` for a 2xx, else the failure with its judgement: 5xx, 408 and
    /// 429 are worth repeating, as is 423 Locked — somebody holds the
    /// resource now and will let go. The rest is ours and will not change.
    ///
    /// # Errors
    /// Any status outside 2xx.
    pub fn judge(&self) -> Result<()> {
        let code = self.status;
        if (200..300).contains(&code) {
            return Ok(());
        }
        Err(TransportError {
            message: format!("the server answered {code}"),
            retryable: code >= 500 || matches!(code, 408 | 423 | 429),
        })
    }
}

fn find<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Write `request` to `authority` with the length its body has.
///
/// # Errors
/// Where the connection broke.
pub fn write_request(stream: &mut impl Write, authority: &str, request: &Request) -> Result<()> {
    let first = format!(
        "{} {} HTTP/1.1\r\nHost: {authority}",
        request.method, request.path
    );
    let head = head(&first, &request.headers, request.body.len());
    write(stream, head.as_bytes(), &request.body, "the request")
}

/// Write `response` with the length its body has.
///
/// # Errors
/// Where the connection broke.
pub fn write_response(stream: &mut impl Write, response: &Response) -> Result<()> {
    let first = format!("HTTP/1.1 {} {}", response.status, reason(response.status));
    let head = head(&first, &response.headers, response.body.len());
    write(stream, head.as_bytes(), &response.body, "the response")
}

/// The head: its first line, every header, the length, the blank line.
fn head(first: &str, headers: &[(String, String)], length: usize) -> String {
    use std::fmt::Write as _;
    let mut head = format!("{first}\r\n");
    for (name, value) in headers {
        write!(head, "{name}: {value}\r\n").expect("writing to a String cannot fail");
    }
    write!(head, "Content-Length: {length}\r\n\r\n").expect("writing to a String cannot fail");
    head
}

fn write(stream: &mut impl Write, head: &[u8], body: &[u8], what: &str) -> Result<()> {
    stream
        .write_all(head)
        .map_err(|e| classify(&format!("writing {what} head"), &e))?;
    stream
        .write_all(body)
        .map_err(|e| classify(&format!("writing {what} body"), &e))?;
    stream
        .flush()
        .map_err(|e| classify(&format!("flushing {what}"), &e))
}

/// Read one request, or `None` when the peer closed between requests.
///
/// # Errors
/// A request line that is not `METHOD path HTTP/1.1`, a body whose length
/// is missing or over [`MAX_BODY`], or a connection that broke.
pub fn read_request(reader: &mut impl BufRead) -> Result<Option<Request>> {
    let head = read_head(reader)?;
    let Some(line) = head.first() else {
        return Ok(None);
    };
    let mut words = line.split_whitespace();
    let (Some(method), Some(path)) = (words.next(), words.next()) else {
        return Err(protocol_error(format!(
            "a request line Xmip cannot read: {line}"
        )));
    };
    let body = read_body(reader, &head)?;
    Ok(Some(Request {
        method: method.to_string(),
        path: path.to_string(),
        headers: headers(&head),
        body,
    }))
}

/// Read one response.
///
/// # Errors
/// A status line Xmip cannot read, a chunked body, a body over
/// [`MAX_BODY`], or a connection that closed before answering.
pub fn read_response(reader: &mut impl BufRead) -> Result<Response> {
    let head = read_head(reader)?;
    let Some(line) = head.first() else {
        return Err(protocol_error("the server closed without answering"));
    };
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| protocol_error(format!("a status line Xmip cannot read: {line}")))?;
    let body = if status == 204 || status == 304 {
        Vec::new()
    } else {
        read_body(reader, &head)?
    };
    Ok(Response {
        status,
        headers: headers(&head),
        body,
    })
}

fn headers(head: &[String]) -> Vec<(String, String)> {
    head.iter()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn read_body(reader: &mut impl Read, head: &[String]) -> Result<Vec<u8>> {
    if header(head, "transfer-encoding").is_some_and(|value| value.contains("chunked")) {
        return Err(protocol_error(
            "a chunked body, which this transport does not read",
        ));
    }
    let Some(value) = header(head, "content-length") else {
        return Ok(Vec::new());
    };
    let length: usize = value
        .parse()
        .map_err(|_| protocol_error(format!("a content-length that is not a number: {value}")))?;
    if length > MAX_BODY {
        return Err(protocol_error(format!(
            "a body of {length} bytes, over the {MAX_BODY} byte limit"
        )));
    }
    let mut bytes = vec![0u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|e| classify("reading the body", &e))?;
    Ok(bytes)
}

/// The phrase a status is written with.
#[must_use]
pub const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        423 => "Locked",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_a_response_round_trip() {
        let request = Request::new("PUT", "/orders/1.edi")
            .with_header("Depth", "1")
            .with_body(b"UNA:+.? '");
        let mut wire = Vec::new();
        write_request(&mut wire, "dav.example:8080", &request).expect("write");
        let text = String::from_utf8_lossy(&wire).into_owned();
        assert!(text.starts_with("PUT /orders/1.edi HTTP/1.1\r\nHost: dav.example:8080\r\n"));
        assert!(text.contains("Content-Length: 9\r\n\r\nUNA"));
        let back = read_request(&mut wire.as_slice())
            .expect("read")
            .expect("one");
        assert_eq!(back.method, "PUT");
        assert_eq!(back.path, "/orders/1.edi");
        assert_eq!(back.header("depth"), Some("1"));
        assert_eq!(back.header("host"), Some("dav.example:8080"));
        assert_eq!(back.body, request.body);
        let response = Response::new(207)
            .with_header("Lock-Token", "<x>")
            .with_body(b"<a/>");
        let mut wire = Vec::new();
        write_response(&mut wire, &response).expect("write");
        assert!(wire.starts_with(b"HTTP/1.1 207 Multi-Status\r\n"));
        let back = read_response(&mut wire.as_slice()).expect("read");
        assert_eq!(back.status, 207);
        assert_eq!(back.header("lock-token"), Some("<x>"));
        assert_eq!(back.body, b"<a/>");
        assert!(back.judge().is_ok());
    }

    #[test]
    fn what_is_not_http_is_refused_and_a_lock_is_worth_waiting_for() {
        assert!(read_request(&mut &b""[..]).expect("closed").is_none());
        assert!(read_request(&mut &b"nonsense\r\n\r\n"[..]).is_err());
        assert!(read_response(&mut &b""[..]).is_err(), "closed early");
        assert!(read_response(&mut &b"HTTP/1.1 x\r\n\r\n"[..]).is_err());
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert!(read_response(&mut &chunked[..]).is_err(), "chunked");
        let short = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab";
        assert!(read_response(&mut &short[..]).is_err(), "short body");
        let none = read_response(&mut &b"HTTP/1.1 204 No Content\r\n\r\n"[..]).expect("204");
        assert!(none.body.is_empty());
        assert!(Response::new(423).judge().expect_err("locked").retryable);
        assert!(Response::new(503).judge().expect_err("server").retryable);
        assert!(!Response::new(404).judge().expect_err("missing").retryable);
    }
}
