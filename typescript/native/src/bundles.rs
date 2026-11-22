use crate::artifacts::{
    read_artifacts, read_assets, read_dependencies, write_artifacts, write_assets,
    write_dependencies,
};
use crate::content::Captured;
use crate::convert::{ValueTree, absolute_path};
use crate::errors::{BindingError, BindingResult, status, tagged};
use crate::guard::{guarded, on_large_stack};
use crate::pool::spawn_promise;
use flyrail::{Bundle, BundleEntry, BundleIdentity, SkillSpec, Value};
use napi::bindgen_prelude::{Object, PromiseRaw, Uint8Array, Unknown};
use napi::{Env, JsValue};
use napi_derive::napi;
use std::sync::Arc;

#[napi(object)]
pub struct JsBundleIdentity {
    pub id: String,
    pub version: String,
}

#[napi(object)]
pub struct JsSkillSpec {
    pub name: String,
    pub path: String,
    pub executables: Vec<String>,
}

#[napi(object)]
pub struct JsBundleEntry {
    pub path: String,
    pub kind: String,
    pub data: Option<Uint8Array>,
    pub executable: bool,
}

#[napi(object)]
pub struct JsBundle {
    pub id: String,
    pub version: String,
    pub skills: Vec<JsSkillSpec>,
    pub entries: Vec<JsBundleEntry>,
    pub content_digest: String,
    pub source_roots: Vec<String>,
    pub schema_version: u32,
}

#[napi]
pub struct NativeBundleIdentity {
    pub(crate) inner: BundleIdentity,
}

#[napi]
impl NativeBundleIdentity {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsBundleIdentity {
        JsBundleIdentity {
            id: self.inner.id().to_owned(),
            version: self.inner.version().to_owned(),
        }
    }
}

#[napi]
pub struct NativeSkillSpec {
    pub(crate) inner: SkillSpec,
}

fn skill_view(skill: &SkillSpec) -> JsSkillSpec {
    JsSkillSpec {
        name: skill.name().to_owned(),
        path: skill.path().to_owned(),
        executables: skill.executables().to_vec(),
    }
}

#[napi]
impl NativeSkillSpec {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsSkillSpec {
        skill_view(&self.inner)
    }
}

#[napi]
pub struct NativeBundle {
    pub(crate) inner: Arc<Bundle>,
}

pub(crate) fn entry_view(entry: &BundleEntry) -> JsBundleEntry {
    JsBundleEntry {
        path: entry.path().to_owned(),
        kind: String::from(if entry.is_directory() {
            "directory"
        } else {
            "file"
        }),
        data: entry.data().map(|data| Uint8Array::from(data.to_vec())),
        executable: entry.executable(),
    }
}

#[napi]
impl NativeBundle {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsBundle {
        let bundle = &self.inner;
        JsBundle {
            id: bundle.id().to_owned(),
            version: bundle.version().to_owned(),
            skills: bundle.skills().iter().map(skill_view).collect(),
            entries: bundle.entries().iter().map(entry_view).collect(),
            content_digest: bundle.content_digest().to_owned(),
            source_roots: bundle
                .source_roots()
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            schema_version: u32::from(bundle.schema_version()),
        }
    }
}

#[napi]
impl NativeBundle {
    #[napi(
        catch_unwind,
        ts_return_type = "NativeResult<{ artifacts: ModelWire[]; assets: ModelWire[]; dependencies: ModelWire[] }>"
    )]
    pub fn models<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        tagged(
            env,
            guarded(|| {
                status((|| {
                    let mut object = Object::new(env)?;
                    object.set("artifacts", write_artifacts(env, self.inner.artifacts())?)?;
                    object.set("assets", write_assets(env, self.inner.assets())?)?;
                    object.set(
                        "dependencies",
                        write_dependencies(env, self.inner.dependencies())?,
                    )?;
                    Ok(object)
                })())
            }),
        )
    }
}

#[napi(
    js_name = "bundleIdentity",
    catch_unwind,
    ts_return_type = "NativeResult<NativeBundleIdentity>"
)]
pub fn bundle_identity(env: &Env, id: String, version: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            Ok(NativeBundleIdentity {
                inner: BundleIdentity::new(id, version)?,
            })
        }),
    )
}

#[napi(
    js_name = "bundleSkillSpec",
    catch_unwind,
    ts_return_type = "NativeResult<NativeSkillSpec>"
)]
pub fn bundle_skill_spec(
    env: &Env,
    name: String,
    path: String,
    executables: Vec<String>,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            Ok(NativeSkillSpec {
                inner: SkillSpec::new(name, path, executables)?,
            })
        }),
    )
}

pub(crate) fn entry_from_js(input: JsBundleEntry) -> BindingResult<BundleEntry> {
    entry_from_parts(
        input.path,
        &input.kind,
        input.data.map(|data| data.to_vec()),
        input.executable,
    )
}

pub(crate) fn entry_from_parts(
    path: String,
    kind: &str,
    data: Option<Vec<u8>>,
    executable: bool,
) -> BindingResult<BundleEntry> {
    match (kind, data) {
        ("file", Some(data)) => Ok(BundleEntry::new(path, Some(data), executable)?),
        ("directory", None) => Ok(BundleEntry::new(path, None, executable)?),
        ("file", None) => Err(BindingError::Argument(String::from(
            "file entries require data",
        ))),
        ("directory", Some(_)) => Err(BindingError::Argument(String::from(
            "directory entries cannot carry data",
        ))),
        _ => Err(BindingError::Argument(String::from(
            "entry kind must be file or directory",
        ))),
    }
}

fn entries_from_js(inputs: Vec<JsBundleEntry>) -> BindingResult<Vec<BundleEntry>> {
    inputs.into_iter().map(entry_from_js).collect()
}

fn wrap(bundle: Bundle) -> NativeBundle {
    NativeBundle {
        inner: Arc::new(bundle),
    }
}

#[napi(
    js_name = "bundleFromDirectory",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeBundle>>"
)]
pub fn bundle_from_directory(
    env: &Env,
    root: String,
) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    spawn_promise(
        env,
        None,
        move || Ok(Bundle::from_directory(absolute_path(&root)?)?),
        |_, bundle| Ok(wrap(bundle)),
    )
}

#[napi(
    js_name = "bundleFromZip",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeBundle>>"
)]
pub fn bundle_from_zip(
    env: &Env,
    path: String,
    resource: String,
) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    spawn_promise(
        env,
        None,
        move || Ok(Bundle::from_zip(absolute_path(&path)?, &resource)?),
        |_, bundle| Ok(wrap(bundle)),
    )
}

#[napi(
    js_name = "bundleFromMemory",
    catch_unwind,
    ts_return_type = "NativeResult<NativeBundle>"
)]
pub fn bundle_from_memory(
    env: &Env,
    manifest: Uint8Array,
    entries: Vec<JsBundleEntry>,
) -> napi::Result<Object<'_>> {
    let manifest = manifest.to_vec();
    let bundle = entries_from_js(entries).and_then(|entries| {
        on_large_stack(move || Ok(wrap(Bundle::from_memory(&manifest, entries)?)))
    });
    tagged(env, bundle)
}

#[napi(
    js_name = "bundleFromMemoryValue",
    catch_unwind,
    ts_args_type = "manifest: NativeValue, entries: JsBundleEntry[]",
    ts_return_type = "NativeResult<NativeBundle>"
)]
pub fn bundle_from_memory_value<'env>(
    env: &'env Env,
    manifest: Unknown<'_>,
    entries: Vec<JsBundleEntry>,
) -> napi::Result<Object<'env>> {
    let bundle = ValueTree::read(env.raw(), manifest.raw())
        .map(|tree| tree.0)
        .and_then(|manifest: Value| entries_from_js(entries).map(|entries| (manifest, entries)))
        .and_then(|(manifest, entries)| {
            on_large_stack(move || Ok(wrap(Bundle::from_memory_value(&manifest, entries)?)))
        });
    tagged(env, bundle)
}

#[napi(
    js_name = "bundleFromArtifacts",
    catch_unwind,
    ts_args_type = "id: string, version: string, wire: { artifacts: ModelWire[]; assets: ModelWire[]; dependencies: ModelWire[] }",
    ts_return_type = "NativeResult<NativeBundle>"
)]
pub fn bundle_from_artifacts<'env>(
    env: &'env Env,
    id: String,
    version: String,
    wire: Unknown<'env>,
) -> napi::Result<Object<'env>> {
    let bundle = Captured::read(*env, wire).and_then(|captured| {
        on_large_stack(move || {
            let wire = captured.wire();
            let artifacts = read_artifacts(&wire, "artifacts")?;
            let assets = read_assets(&wire, "assets")?;
            let dependencies = read_dependencies(&wire, "dependencies")?;
            Ok(wrap(Bundle::from_artifacts(
                BundleIdentity::new(id, version)?,
                artifacts,
                assets,
                dependencies,
            )?))
        })
    });
    tagged(env, bundle)
}
