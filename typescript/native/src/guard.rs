use crate::errors::{BindingError, BindingResult};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;
use std::thread;

pub(crate) const LARGE_STACK_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| String::from("native panic"))
}

pub(crate) fn guarded<T>(work: impl FnOnce() -> BindingResult<T>) -> BindingResult<T> {
    catch_unwind(AssertUnwindSafe(work))
        .unwrap_or_else(|payload| Err(BindingError::Panic(panic_message(payload.as_ref()))))
}

pub(crate) fn on_large_stack<T: Send>(
    work: impl FnOnce() -> BindingResult<T> + Send,
) -> BindingResult<T> {
    thread::scope(|scope| {
        let handle = thread::Builder::new()
            .name(String::from("flyrail-stack"))
            .stack_size(LARGE_STACK_BYTES)
            .spawn_scoped(scope, || guarded(work))
            .map_err(|error| {
                BindingError::Native(format!("could not start a native thread: {error}"))
            })?;
        handle
            .join()
            .unwrap_or_else(|payload| Err(BindingError::Panic(panic_message(payload.as_ref()))))
    })
}

pub(crate) fn install_silent_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| std::panic::set_hook(Box::new(|_| {})));
}
