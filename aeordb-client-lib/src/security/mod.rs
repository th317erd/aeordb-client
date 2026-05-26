//! Security primitives used by the client.
//!
//! Currently just the ed25519 trust store for self-update manifest
//! verification (see `crate::update`). The plugin-signature concerns
//! that xenocept-client also keeps in here aren't needed — aeordb-client
//! has no plugin system.

pub mod sig;
