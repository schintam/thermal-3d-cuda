//! Keepers Studio engine: import, catalog, culling previews and scores,
//! non-destructive develop and export. No network access, no telemetry.

pub mod bursts;
pub mod catalog;
pub mod develop;
pub mod embedded;
pub mod export;
pub mod formats;
pub mod heic;
pub mod library;
pub mod meta;
pub mod preview;
pub mod scoring;
pub mod sidecar;
pub mod sources;
#[doc(hidden)]
pub mod testkit;
pub mod util;

pub use library::{Event, Library};
