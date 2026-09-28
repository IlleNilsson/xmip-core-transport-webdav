# xmip-core-transport-webdav

WebDAV transport: a collection over HTTP — PROPFIND lists, GET and DELETE take, PUT delivers, and LOCK is the native claim, so two nodes never take one file twice. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

Every method — a send's PUT, a receive's PROPFIND, LOCK, GET and DELETE, a claim's LOCK and UNLOCK — goes on the http technology's kept connections (`endpoint::Connections`): opened once per server and reused by every method after, replaced where the server closed one. Until 2026-09-28 every send, receive and claim connected anew.

The collection is kept as written and read by `net::Endpoint` in [xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net) under the schemes this technology declares — `webdav://` is `http://`, `webdavs://` is `https://`. Until 2026-09-28 an `as_http` function rewrote the URL before it was read.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
