//! The module's timing program (`doc/governance/rust-style.md` section
//! 5c): this transport held to how long Xmip takes — the millisecond rule
//! (`CONTRIBUTING.md`, *Rust is here for one reason*). A plain `cargo test`
//! leaves it out; the landing runs it after every other test of the module,
//! alone, one test at a time
//! (`cargo test --test measured --all-features -- --test-threads=1`), so
//! what is measured is Xmip's work and not its neighbors':
//!
//! - a thousand listings on one connection take a millisecond each at most
//!   beyond their wakes, round by round beside a plain wake, the fastest of
//!   five (`xcore::latency::Rounds`).

use std::collections::BTreeMap;
use std::time::Duration;

use transport::Configured;
use transport::Transport;
use xcore::latency::Rounds;
use xmip_core_transport_webdav::{Store, WebDavTransport};

/// What a listing wakes where a plain wake wakes one: the far end at the
/// PROPFIND, and this end at its answer.
const WAKES: u32 = 2;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn a_thousand_listings_take_a_millisecond_each_at_most() {
    const RECEIVES: usize = 1000;
    let far_end = WebDavTransport::new("webdav://127.0.0.1:0/orders").timing_out_after(secs(2));
    let (listener, address) = far_end.bind().expect("binding");
    let receiver = std::thread::spawn(move || {
        let near =
            WebDavTransport::new(format!("webdav://{address}/orders")).timing_out_after(secs(5));
        let mut rounds = Rounds::crossing(WAKES);
        for _ in 0..RECEIVES {
            assert!(rounds.round(|| near.receive())?.is_empty());
        }
        Ok::<_, transport::TransportError>(rounds)
    });
    let mut store = Store::default();
    store.files.insert("/orders".to_string(), BTreeMap::new());
    let mut session = far_end
        .accept_one(&listener)
        .expect("accepting")
        .with_store(store);
    session.serve().expect("serving");
    let rounds = receiver.join().expect("thread").expect("listed");
    rounds.hold("a listing");
}
