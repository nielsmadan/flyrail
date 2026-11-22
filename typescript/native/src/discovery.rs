use crate::pool::{Built, spawn_promise};
use flyrail::AgentPresence;
use napi::Env;
use napi::bindgen_prelude::{Object, PromiseRaw};
use napi_derive::napi;
use std::path::PathBuf;

use crate::targets::lossy;

#[allow(clippy::ptr_arg)]
fn write_presences<'env>(
    env: &'env Env,
    presences: &Vec<AgentPresence>,
) -> napi::Result<Object<'env>> {
    let items = presences
        .iter()
        .map(|presence| {
            let mut object = Object::new(env)?;
            object.set("agent", presence.agent().as_str())?;
            object.set("executable", presence.executable().map(lossy))?;
            object.set("application", presence.application().map(lossy))?;
            Ok(object)
        })
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("items", items)?;
    Ok(object)
}

#[napi(
    js_name = "discoveryDetectAgents",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ items: AgentPresenceWire[] }>>"
)]
pub fn discovery_detect_agents(
    env: &Env,
    search_path: String,
    applications: Vec<String>,
) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    let applications = applications
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    spawn_promise(
        env,
        None,
        move || {
            Ok(flyrail::detect_agents(
                Some(&search_path),
                Some(&applications),
            )?)
        },
        |_, presences| Ok(Built::new(presences, write_presences)),
    )
}
