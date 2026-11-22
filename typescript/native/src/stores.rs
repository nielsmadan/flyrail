use crate::convert::absolute_path;
use crate::errors::tagged;
use crate::guard::guarded;
use crate::pool::spawn_promise;
use crate::targets::lossy;
use flyrail::{DirectoryStore, MemoryStore, StateStore};
use napi::Env;
use napi::bindgen_prelude::{Object, PromiseRaw, Uint8Array};
use napi_derive::napi;
use std::sync::Arc;

#[napi(object)]
pub struct JsStoreLocation {
    pub location: Option<String>,
    pub staging_directory: Option<String>,
}

#[napi]
pub struct NativeStateStore {
    pub(crate) inner: Arc<dyn StateStore>,
}

#[napi]
impl NativeStateStore {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsStoreLocation {
        JsStoreLocation {
            location: self.inner.location().map(lossy),
            staging_directory: self.inner.staging_directory().as_deref().map(lossy),
        }
    }

    #[napi(
        catch_unwind,
        ts_return_type = "Promise<NativeResult<Uint8Array | null>>"
    )]
    pub fn read<'env>(
        &self,
        env: &'env Env,
        key: String,
    ) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
        let store = Arc::clone(&self.inner);
        spawn_promise(
            env,
            None,
            move || Ok(store.read(&key)?),
            |_, value| Ok(value.map(Uint8Array::from)),
        )
    }

    #[napi(catch_unwind, ts_return_type = "Promise<NativeResult<boolean>>")]
    pub fn write<'env>(
        &self,
        env: &'env Env,
        key: String,
        value: Uint8Array,
        expected: Option<Uint8Array>,
    ) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
        let store = Arc::clone(&self.inner);
        let (value, expected) = (value.to_vec(), expected.map(|expected| expected.to_vec()));
        spawn_promise(
            env,
            None,
            move || Ok(store.write(&key, &value, expected.as_deref())?),
            |_, written| Ok(written),
        )
    }

    #[napi(catch_unwind, ts_return_type = "Promise<NativeResult<boolean>>")]
    pub fn delete<'env>(
        &self,
        env: &'env Env,
        key: String,
        expected: Uint8Array,
    ) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
        let store = Arc::clone(&self.inner);
        let expected = expected.to_vec();
        spawn_promise(
            env,
            None,
            move || Ok(store.delete(&key, &expected)?),
            |_, deleted| Ok(deleted),
        )
    }

    #[napi(catch_unwind, ts_return_type = "Promise<NativeResult<string[]>>")]
    pub fn keys<'env>(
        &self,
        env: &'env Env,
        prefix: String,
    ) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
        let store = Arc::clone(&self.inner);
        spawn_promise(
            env,
            None,
            move || Ok(store.keys(&prefix)?),
            |_, keys| Ok(keys),
        )
    }
}

#[napi(
    js_name = "storeOpenDirectory",
    catch_unwind,
    ts_return_type = "NativeResult<NativeStateStore>"
)]
pub fn store_open_directory(env: &Env, path: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let store: Arc<dyn StateStore> = Arc::new(DirectoryStore::new(absolute_path(&path)?)?);
            Ok(NativeStateStore { inner: store })
        }),
    )
}

#[napi(
    js_name = "storeNewMemory",
    catch_unwind,
    ts_return_type = "NativeResult<NativeStateStore>"
)]
pub fn store_new_memory(env: &Env) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
            Ok(NativeStateStore { inner: store })
        }),
    )
}

#[napi(
    js_name = "storeValidateKey",
    catch_unwind,
    ts_return_type = "NativeResult<undefined>"
)]
pub fn store_validate_key(env: &Env, key: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| Ok(flyrail::stores::validate_key(&key, "state key")?)),
    )
}

#[napi(
    js_name = "storeValidateLockName",
    catch_unwind,
    ts_return_type = "NativeResult<undefined>"
)]
pub fn store_validate_lock_name(env: &Env, name: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| Ok(flyrail::stores::validate_key(&name, "lock name")?)),
    )
}
