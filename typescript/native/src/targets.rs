use crate::content::parse;
use crate::convert::absolute_path;
use crate::errors::tagged;
use crate::guard::guarded;
use flyrail::{Agent, Target};
use napi::Env;
use napi::bindgen_prelude::Object;
use napi_derive::napi;

#[napi(js_name = "targetsAgents", catch_unwind, ts_return_type = "Agent[]")]
pub fn targets_agents() -> Vec<String> {
    flyrail::Agent::ALL
        .iter()
        .map(|agent| agent.as_str().to_owned())
        .collect()
}

#[napi(object)]
pub struct JsPair {
    pub key: String,
    pub value: String,
}

#[napi(object)]
pub struct JsTarget {
    pub root: String,
    pub agent: Option<String>,
    pub scope: String,
    pub home: Option<String>,
    pub environment: Vec<JsPair>,
}

pub(crate) fn pair_view((key, value): &(String, String)) -> JsPair {
    JsPair {
        key: key.clone(),
        value: value.clone(),
    }
}

pub(crate) fn pair_parts(pair: JsPair) -> (String, String) {
    (pair.key, pair.value)
}

pub(crate) fn lossy(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

#[napi]
pub struct NativeTarget {
    pub(crate) inner: Target,
}

#[napi]
#[allow(clippy::use_self)]
impl NativeTarget {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsTarget {
        JsTarget {
            root: lossy(self.inner.root()),
            agent: self.inner.agent().map(|agent| agent.as_str().to_owned()),
            scope: self.inner.scope().as_str().to_owned(),
            home: self.inner.home().map(lossy),
            environment: self.inner.environment().iter().map(pair_view).collect(),
        }
    }

    #[napi(catch_unwind)]
    pub fn equals(&self, other: &NativeTarget) -> bool {
        self.inner == other.inner
    }
}

#[napi(
    js_name = "targetDirectory",
    catch_unwind,
    ts_return_type = "NativeResult<NativeTarget>"
)]
pub fn target_directory(env: &Env, root: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            Ok(NativeTarget {
                inner: Target::directory(absolute_path(&root)?)?,
            })
        }),
    )
}

#[napi(
    js_name = "targetProject",
    catch_unwind,
    ts_return_type = "NativeResult<NativeTarget>"
)]
pub fn target_project(env: &Env, agent: String, root: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            Ok(NativeTarget {
                inner: Target::project(parse::<Agent>(&agent)?, absolute_path(&root)?)?,
            })
        }),
    )
}

#[napi(
    js_name = "targetUser",
    catch_unwind,
    ts_return_type = "NativeResult<NativeTarget>"
)]
pub fn target_user(
    env: &Env,
    agent: String,
    home: String,
    environment: Option<Vec<JsPair>>,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let agent = parse::<Agent>(&agent)?;
            let home = absolute_path(&home)?;
            Ok(NativeTarget {
                inner: match environment {
                    Some(pairs) => Target::user_with_home_and_environment(
                        agent,
                        home,
                        pairs.into_iter().map(pair_parts),
                    )?,
                    None => Target::user_with_home(agent, home)?,
                },
            })
        }),
    )
}
