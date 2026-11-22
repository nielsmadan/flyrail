use crate::targets::lossy;
use flyrail::{
    Installation, InventoryEntry, Observation, ResourceKind, Target, TargetError, TargetInspection,
    TargetResult,
};
use napi::Env;
use napi::bindgen_prelude::{BigInt, Object};
use std::path::PathBuf;

pub(crate) fn write_target_error<'env>(
    env: &'env Env,
    error: Option<&TargetError>,
) -> napi::Result<Option<Object<'env>>> {
    error
        .map(|error| {
            let mut object = Object::new(env)?;
            object.set("code", error.code().as_str())?;
            object.set("message", error.message())?;
            object.set("path", error.path().map(lossy))?;
            object.set("errno", error.errno())?;
            Ok(object)
        })
        .transpose()
}

pub(crate) fn path_texts(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|path| lossy(path)).collect()
}

pub(crate) const fn resource_kind(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Document => "document",
        ResourceKind::Tree => "tree",
        ResourceKind::SkillContainer => "skill-container",
    }
}

fn write_inventory_entry<'env>(
    env: &'env Env,
    entry: &InventoryEntry,
) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set("path", entry.path())?;
    object.set("directory", entry.is_directory())?;
    object.set("size", entry.size().map(BigInt::from))?;
    object.set("sha256", entry.sha256())?;
    object.set("executable", entry.executable())?;
    Ok(object)
}

fn write_installation<'env>(
    env: &'env Env,
    installation: &Installation,
) -> napi::Result<Object<'env>> {
    let entries = installation
        .entries()
        .iter()
        .map(|entry| write_inventory_entry(env, entry))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("bundleId", installation.bundle_id())?;
    object.set("version", installation.version())?;
    object.set("contentDigest", installation.content_digest())?;
    object.set("transactionId", installation.transaction_id())?;
    object.set("entries", entries)?;
    Ok(object)
}

fn write_observation<'env>(
    env: &'env Env,
    observation: &Observation,
) -> napi::Result<Object<'env>> {
    let installed = observation
        .installed()
        .map(|installation| write_installation(env, installation))
        .transpose()?;
    let installations = observation
        .installations()
        .iter()
        .map(|installation| write_installation(env, installation))
        .collect::<napi::Result<Vec<_>>>()?;
    let modifications = observation
        .modifications()
        .iter()
        .map(|modification| {
            let mut object = Object::new(env)?;
            object.set("bundleId", modification.bundle_id())?;
            object.set("path", modification.path())?;
            object.set("kind", modification.kind().as_str())?;
            Ok(object)
        })
        .collect::<napi::Result<Vec<_>>>()?;
    let conflicts = observation
        .conflicts()
        .iter()
        .map(|conflict| {
            let mut object = Object::new(env)?;
            object.set("path", conflict.path())?;
            object.set("owner", conflict.owner())?;
            Ok(object)
        })
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("state", observation.state().as_str())?;
    object.set("installed", installed)?;
    object.set("installations", installations)?;
    object.set("versionMatches", observation.version_matches())?;
    object.set(
        "recordedContentMatches",
        observation.recorded_content_matches(),
    )?;
    object.set("contentMatches", observation.content_matches())?;
    object.set("modifications", modifications)?;
    object.set("conflicts", conflicts)?;
    object.set("error", write_target_error(env, observation.error())?)?;
    object.set("recoveryPaths", path_texts(observation.recovery_paths()))?;
    object.set("isCurrent", observation.is_current())?;
    Ok(object)
}

fn write_alias(object: &mut Object<'_>, alias_of: Option<usize>) -> napi::Result<()> {
    let index = alias_of
        .map(|index| {
            u32::try_from(index)
                .map_err(|_| napi::Error::from_reason("target index exceeds the u32 range"))
        })
        .transpose()?;
    object.set("aliasOf", index)
}

#[allow(clippy::ptr_arg)]
pub(crate) fn write_inspections<'env>(
    env: &'env Env,
    inspections: &Vec<TargetInspection<Target>>,
) -> napi::Result<Object<'env>> {
    let items = inspections
        .iter()
        .map(|inspection| {
            let mut object = Object::new(env)?;
            object.set("root", lossy(inspection.root()))?;
            write_alias(&mut object, inspection.alias_of())?;
            object.set(
                "observation",
                write_observation(env, inspection.observation())?,
            )?;
            Ok(object)
        })
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("items", items)?;
    Ok(object)
}

#[allow(clippy::ptr_arg)]
pub(crate) fn write_target_results<'env>(
    env: &'env Env,
    results: &Vec<TargetResult<Target>>,
) -> napi::Result<Object<'env>> {
    let items = results
        .iter()
        .map(|result| {
            let mut object = Object::new(env)?;
            object.set("root", lossy(result.root()))?;
            write_alias(&mut object, result.alias_of())?;
            object.set("status", result.status().as_str())?;
            object.set("observation", write_observation(env, result.observation())?)?;
            object.set("error", write_target_error(env, result.error())?)?;
            object.set("recoveryPaths", path_texts(result.recovery_paths()))?;
            Ok(object)
        })
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("items", items)?;
    Ok(object)
}
