//! What a Location of this technology is configured with: its declaration,
//! which `Configured` reads, built with the crate's `declaration` feature
//! alone and the code behind the default (ADR-0064, amendments 2026-10-22
//! and 2026-11-26), for the library the surfaces load to carry (open
//! problem 48).

use std::time::Duration;

use xcore::settings::{Applies, Fixed, Kind, Presence, Setting, Settings};

/// How long a lock is asked to hold where `lock_timeout` is left out: ten
/// minutes, longer than any receive cycle should take.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(600);

/// Every setting a Location takes beyond its address, the collection's
/// URL, `webdav://host:port/orders`: the collection a Receive Location
/// takes from and a Send Location's names resolve inside.
pub const SETTINGS: &Settings = &Settings {
    technology: env!("CARGO_PKG_NAME"),
    settings: &[
        transport::configured::timeout(
            Applies::Both,
            None,
            "How long a server that stops mid-answer is waited on; unbounded \
             when left out.",
        ),
        Setting {
            name: "lock_timeout",
            kind: Kind::Duration,
            presence: Presence::Default(Fixed::Duration(LOCK_TIMEOUT)),
            meaning: "How long a lock is asked to hold (whole seconds; the server may \
                      grant less): the one that claims a received member, or a claim's. A \
                      member whose receive cycle never ends — its node stopped — is taken \
                      again once it lapses, so it must outlast the longest cycle; 600s \
                      when left out.",
            applies: Applies::Both,
        },
    ],
};

/// `webdav://`, and `webdavs://`, which TLS guards.
pub const SCHEMES: &[&str] = &["webdav", "webdavs"];
