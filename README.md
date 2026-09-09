# xmip-core-transport-webdav

WebDAV transport: a collection over HTTP — PROPFIND lists, GET and DELETE take, PUT delivers, and LOCK is the native claim, so two nodes never take one file twice. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
