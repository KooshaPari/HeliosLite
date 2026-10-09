// clippy 1.99 raises `redundant_field_names` against the `diesel::QueryableByName`
// derive's generated struct initializers, pointing at the field-declaration span
// (the diagnostic carries no macro-expansion context) and suggesting a rewrite
// that would not compile. Allow it at crate scope; see forge_infra/src/env.rs.
#![allow(clippy::redundant_field_names)]
mod agent;
mod agent_definition;
mod codec;
mod context_engine;
mod conversation;
mod daemon_repo;
mod database;
mod forge_repo;
mod fs_snap;
mod fuzzy_search;
mod provider;
mod skill;
mod validation;

mod proto_generated {
    tonic::include_proto!("forge.v1");
}

// Only expose forge_repo container
pub use conversation::{
    ForgeSnapshot, ForgeSnapshotManifest, ForgeSnapshotRow, SNAPSHOT_CONTRACT_VERSION,
    export_forge_snapshot, publish_snapshot_atomic,
};
pub use forge_repo::*;
