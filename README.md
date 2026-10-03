# xmip-core-transport-webdav

WebDAV transport: a collection over HTTP — PROPFIND lists, GET and DELETE take, PUT delivers, and LOCK is the native claim, so two nodes never take one file twice. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

Every method — a send's PUT, a receive's PROPFIND, LOCK, GET and DELETE, a claim's LOCK and UNLOCK — goes on the http technology's kept connections (`endpoint::Connections`): opened once per server and reused by every method after, replaced where the server closed one. Until 2026-09-28 every send, receive and claim connected anew.

The collection is kept as written and read by `net::Endpoint` in [xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net) under the schemes this technology declares — `webdav://` is `http://`, `webdavs://` is `https://`. Until 2026-09-28 an `as_http` function rewrote the URL before it was read.

## Acknowledged after the receive cycle

A receive lists the collection and LOCKs each member it hands up — the lock is the claim, so no other node takes it while its cycle runs — and deletes nothing. The member is read by a GET when the runtime first pulls its body, one member at a time. On `Accepted` it is deleted with its lock token, which takes the lock with it (RFC 4918 section 9.6). On `Refused` it is deleted the same way: a collection has no place for a refused member, the runtime audited the refusal, and from Message creation on the Stream is kept in Xmip (ADR-0013). On `Failed` it is unlocked and left in the collection for the next receive. A member whose verdict never comes stays locked until the lock lapses and is then taken again.

The lock — a receive's, and a claim's — is asked to hold for `lock_timeout` (a duration, 600 seconds when left out), written as the LOCK's `Timeout: Second-<n>` header (RFC 4918 section 10.7; the server may grant less). It must outlast the longest receive cycle: a lock that lapses mid-cycle lets another node take the member too. The requests are the ones a receive always made — LOCK, GET, DELETE — only later; a failure adds one UNLOCK.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
