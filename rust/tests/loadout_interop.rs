#![allow(clippy::expect_used)]

use flyrail::{
    Agent, Bundle, DirectoryStore, HookRuntime, HookShell, LoadoutDiscovery, LoadoutLocation,
    LoadoutOptions, OperationStatus, Platform, apply_loadout, discover_loadout, inspect_loadout,
    preview_loadout, preview_loadout_removal,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[test]
#[ignore = "run spec/check_loadout_interop.py to provision the fixture"]
fn loadout_interop_producer() {
    let root =
        PathBuf::from(std::env::var_os("FLYRAIL_LOADOUT_INTEROP_ROOT").expect("fixture root"));
    let phase = std::env::var("FLYRAIL_LOADOUT_INTEROP_PHASE").expect("fixture phase");
    let store = Arc::new(DirectoryStore::new(root.join("state")).expect("state store"));
    if phase == "remove" {
        let source =
            PathBuf::from(std::env::var_os("FLYRAIL_LOADOUT_INTEROP_SOURCE").expect("source path"));
        let location =
            LoadoutLocation::new(source.clone(), "default".into(), "local".into(), source)
                .expect("fixture location");
        for bundle_id in ["extra", "interop"] {
            assert!(
                inspect_loadout(bundle_id, &location, store.clone())
                    .expect("inspect before removal")
                    .is_current()
            );
            let plan = preview_loadout_removal(bundle_id, &location, store.clone(), false)
                .expect("preview removal");
            assert!(plan.applicable(), "{bundle_id}: {plan:?}");
            let removed = apply_loadout(&plan, Duration::ZERO);
            assert!(removed.sync_required(), "{bundle_id}: {removed:?}");
            assert_eq!(
                removed.lifecycle().status(),
                OperationStatus::Applied,
                "{bundle_id}: {removed:?}"
            );
        }
        return;
    }

    assert_eq!(phase, "install");
    let node = PathBuf::from(std::env::var_os("FLYRAIL_LOADOUT_INTEROP_NODE").expect("Node path"));
    let runtime = HookRuntime::new(Some(node), Some(HookShell::Posix)).expect("hook runtime");
    let platform = if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    };
    let options = LoadoutOptions {
        platform,
        hook_runtime: Some(&runtime),
        execution_root: Some(&root),
        replace_modified: false,
    };
    for bundle_id in ["interop", "extra"] {
        let bundle = Bundle::from_directory(root.join(bundle_id)).expect("fixture bundle");
        let mut agents = vec![Agent::Claude, Agent::Codex, Agent::Cursor, Agent::Pi];
        if bundle_id == "interop" {
            agents.push(Agent::Opencode);
        }
        for expected in [OperationStatus::Applied, OperationStatus::Unchanged] {
            let found =
                discover_loadout(&root.join("home"), None, &BTreeMap::new()).expect("discover");
            let LoadoutDiscovery::Available(setup) = found else {
                unreachable!("fixture setup: {found:?}");
            };
            let destination = setup
                .select("local", &agents, false)
                .expect("select declared source");
            let plan =
                preview_loadout(&bundle, &destination, store.clone(), options).expect("preview");
            assert!(plan.applicable(), "{bundle_id}: {plan:?}");
            let result = apply_loadout(&plan, Duration::ZERO);
            assert_eq!(
                result.lifecycle().status(),
                expected,
                "{bundle_id}: {result:?}"
            );
            if expected == OperationStatus::Applied {
                assert!(result.sync_required(), "{bundle_id}: {result:?}");
            }
            assert!(
                inspect_loadout(bundle_id, plan.location(), store.clone())
                    .expect("inspect")
                    .is_current()
            );
        }
    }
}
