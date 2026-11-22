#![allow(clippy::redundant_pub_crate, clippy::needless_pass_by_value)]

mod artifacts;
mod authority;
mod bundles;
mod cancel;
mod content;
mod convert;
mod discovery;
mod editing;
mod errors;
mod guard;
mod hooks;
#[cfg(feature = "test-seams")]
mod internal;
mod lifecycle;
mod loadout;
mod observations;
mod pool;
mod render;
mod skills;
mod stores;
mod targets;
mod translation;
mod values;

use napi_derive::napi;

#[napi_derive::module_init]
fn init() {
    guard::install_silent_panic_hook();
}

#[napi(js_name = "nativeVersion", catch_unwind)]
pub const fn native_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
