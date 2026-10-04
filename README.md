# xmip-core-transport-webdav

WebDAV transport: a collection over HTTP — PROPFIND lists, GET and DELETE take, PUT delivers, and LOCK is the native claim, so two nodes never take one file twice. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

Every method — a send's PUT, a receive's PROPFIND, LOCK, GET and DELETE, a claim's LOCK and UNLOCK — goes on the http technology's kept connections (`endpoint::Connections`): opened once per server and reused by every method after, replaced where the server closed one. Until 2026-09-28 every send, receive and claim connected anew.

The collection is kept as written and read by `net::Endpoint` in [xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net) under the schemes this technology declares — `webdav://` is `http://`, `webdavs://` is `https://`. Until 2026-09-28 an `as_http` function rewrote the URL before it was read.

## Acknowledged after the receive cycle

A receive lists the collection and LOCKs each member it hands up — the lock is the claim, so no other node takes it while its cycle runs — and deletes nothing. The member is read by a GET when the runtime first pulls its body, one member at a time. On `Accepted` it is deleted with its lock token, which takes the lock with it (RFC 4918 section 9.6). On `Refused` it is unlocked and left in the collection: a refusal is not a consumption, and a Stream refused at a transport gate was never written to the Ledger, so the member is the only copy. The Location remembers it by its href with its stamp — the `getetag` PROPFIND reports, or else its `getlastmodified` with its `getcontentlength` (`transport::Refused`) — and does not lock or take it again while it lies there unchanged; written again, it is a new arrival. A server that reports none of the three gives no stamp, and there a refused member is taken, and refused, again on every receive — never lost. The memory is the node process's, so a node started again takes it once more. On `Failed` it is unlocked and left in the collection for the next receive. A member whose verdict never comes stays locked until the lock lapses and is then taken again.

The lock — a receive's, and a claim's — is asked to hold for `lock_timeout` (a duration, 600 seconds when left out), written as the LOCK's `Timeout: Second-<n>` header (RFC 4918 section 10.7; the server may grant less). It must outlast the longest receive cycle: a lock that lapses mid-cycle lets another node take the member too. The requests are the ones a receive always made — LOCK, GET, DELETE — only later; a failure or a refusal adds one UNLOCK.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
