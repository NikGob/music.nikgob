# librespot-core 0.8.0 (patched)

Copy of librespot-core 0.8.0 from crates.io (MIT, the librespot contributors) with one change in
`src/http_client.rs`: the rate limiter names `NoOpMiddleware`'s instant type and its clock explicitly. Upstream
leaves it to the default type parameter, which depends on governor's `quanta` feature; librqbit
enables that feature, so the two crates cannot build together without this. Wired in through
`[patch.crates-io]` in the workspace Cargo.toml. Drop this copy once upstream fixes it.
