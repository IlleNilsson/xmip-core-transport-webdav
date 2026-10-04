//! One member a receive locked: read when the runtime first asks for its
//! body, and deleted or unlocked once its receive cycle has ended.
//!
//! The lock is the claim (ADR-0024), taken as the collection is listed, so
//! no other node takes the member while its cycle runs. Nothing else
//! happens until the runtime pulls the body: the GET is made then, one
//! member at a time, rather than every member read into memory as the
//! receive lists them. On [`Verdict::Accepted`] the member is deleted with
//! the lock token — the delete takes the lock with it (RFC 4918 section
//! 9.6). On [`Verdict::Refused`] it is unlocked and left where it lies: a
//! refusal is not a consumption, and the member is the only copy. The
//! receive remembers it with its stamp ([`transport::Refused`]), and does
//! not take it again while it lies there unchanged. On [`Verdict::Failed`]
//! it is unlocked and left where it is for the next receive. A member
//! whose verdict never comes stays locked until the lock's timeout (the
//! `lock_timeout` setting), then is taken again.

use std::io::Read;

use net::Endpoint;
use transport::body::fetched;
use transport::error::Result;
use transport::{Acknowledgement, Verdict};

use http::endpoint::Connections;

use crate::client::Client;

/// Where a locked member is, and the lock that holds it.
#[derive(Clone)]
pub struct Locked {
    pub endpoint: Endpoint,
    pub timeout: Option<std::time::Duration>,
    pub connections: Connections,
    pub href: String,
    pub token: String,
}

impl Locked {
    fn client(&self) -> Client {
        Client::over(&self.endpoint, self.timeout, self.connections.clone())
    }

    /// The member's body, read by a GET when it is first asked for.
    #[must_use]
    pub fn body(&self) -> impl Read + Send + 'static {
        let locked = self.clone();
        fetched(move || locked.client().get(&locked.href))
    }

    /// Delete it on acceptance, unlock it on refusal and on failure.
    #[must_use]
    pub fn acknowledgement(self) -> Acknowledgement {
        Acknowledgement::deferred(move |verdict| self.told(verdict))
    }

    fn told(&self, verdict: Verdict) -> Result<()> {
        let mut client = self.client();
        match verdict {
            Verdict::Accepted => client.delete_held(&self.href, &self.token),
            Verdict::Refused(_) | Verdict::Failed => client.unlock(&self.href, &self.token),
        }
    }
}
