//! Hot-reload entry points (re-exported on engine side).
//!
//! The real poller is [`prism_asset::runtime::HotReloadWatcher`]: it depends only on `std`, so it is
//! always available at runtime; produced [`HotReloadEvent::PakModified`] is then used by host to
//! [`ResourceManager::on_pak_changed`](prism_asset::runtime::ResourceManager::on_pak_changed)
//! replace stale file bytes.
//!
//! **Engine itself does not poll**, nor automatically reload anything. Host (editor / player) needs three steps:
//!
//! 1. Poll `.pak` (or get path directly where `.pak` is rebuilt);
//! 2. `ResourceManager::on_pak_changed(path)` swap reader and refresh resident assets in place;
//! 3. `GpuAssetResolver::invalidate_assets(&mut world)` push entity-side parse results back to
//!    "pending parse"; next frame `prepare_requests` re-uploads — GPU handle is the one rendering uses.
//!
//! Previously this file held a `HotReloadWatcher` returning only `bool` with no call sites, comment said
//! "already wired"; that was false, already deleted (#495). Do not add a second watcher here.

pub use prism_asset::runtime::{HotReloadError, HotReloadEvent, HotReloadWatcher};
