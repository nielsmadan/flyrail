use crate::content::{
    Wire, node, parse, read_boundaries, read_selector, unknown_kind, write_boundaries,
    write_selector,
};
use crate::convert::absolute_path;
use crate::errors::{BindingResult, tagged};
use crate::guard::guarded;
use crate::pool::spawn_promise;
use flyrail::authority::{canonical_destination, transaction_path};
use flyrail::{Claim, ClaimSelector, Ownership, ResourceAuthority};
use napi::Env;
use napi::bindgen_prelude::{Object, PromiseRaw};
use napi_derive::napi;
use std::sync::Arc;

fn read_claim_selector(wire: &Wire<'_>) -> BindingResult<ClaimSelector> {
    match wire.kind()?.as_str() {
        "whole" => Ok(ClaimSelector::Whole),
        "subtree" => Ok(ClaimSelector::Subtree(wire.field("path")?)),
        "boundaries" | "boundaries-marker" => Ok(ClaimSelector::Section(read_boundaries(wire)?)),
        "selector" => Ok(ClaimSelector::Structured(read_selector(wire)?)),
        other => Err(unknown_kind(other)),
    }
}

pub(crate) fn read_claim(wire: &Wire<'_>) -> BindingResult<Claim> {
    Ok(Claim::new(
        parse::<Ownership>(&wire.field::<String>("ownership")?)?,
        read_claim_selector(&wire.child("selector")?)?,
    )?)
}

fn write_claim_selector<'env>(
    env: &'env Env,
    selector: &ClaimSelector,
) -> napi::Result<Object<'env>> {
    match selector {
        ClaimSelector::Whole => node(env, "whole"),
        ClaimSelector::Subtree(path) => {
            let mut object = node(env, "subtree")?;
            object.set("path", path.as_str())?;
            Ok(object)
        }
        ClaimSelector::Section(boundaries) => write_boundaries(env, boundaries),
        ClaimSelector::Structured(selector) => write_selector(env, selector),
    }
}

pub(crate) fn write_claim<'env>(env: &'env Env, claim: &Claim) -> napi::Result<Object<'env>> {
    let mut object = node(env, "claim")?;
    object.set("ownership", claim.kind().as_str())?;
    object.set("selector", write_claim_selector(env, claim.selector())?)?;
    Ok(object)
}

#[napi]
pub struct NativeAuthority {
    pub(crate) inner: Arc<ResourceAuthority>,
}

#[napi]
impl NativeAuthority {
    #[napi(catch_unwind, ts_return_type = "NativeResult<string>")]
    pub fn destination<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        tagged(
            env,
            guarded(|| Ok(self.inner.destination().to_string_lossy().into_owned())),
        )
    }
}

#[napi(
    js_name = "authorityCreate",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeAuthority>>"
)]
pub fn authority_create(
    env: &Env,
    destination: String,
) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    spawn_promise(
        env,
        None,
        move || Ok(ResourceAuthority::new(absolute_path(&destination)?)?),
        |_, authority| {
            Ok(NativeAuthority {
                inner: Arc::new(authority),
            })
        },
    )
}

#[napi(
    js_name = "authorityCanonicalDestination",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<string>>"
)]
pub fn authority_canonical_destination(
    env: &Env,
    path: String,
) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    spawn_promise(
        env,
        None,
        move || Ok(canonical_destination(absolute_path(&path)?)?),
        |_, path| Ok(path.to_string_lossy().into_owned()),
    )
}

#[napi(
    js_name = "authorityTransactionPath",
    catch_unwind,
    ts_return_type = "NativeResult<string>"
)]
pub fn authority_transaction_path(
    env: &Env,
    destination: String,
    transaction_id: String,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let path = transaction_path(&absolute_path(&destination)?, &transaction_id)?;
            Ok(path.to_string_lossy().into_owned())
        }),
    )
}
