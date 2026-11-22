use crate::cancel::{CancelToken, run_cancellable};
use crate::convert::{ValueTree, duration_ms};
use crate::errors::{BindingError, BindingResult, tagged};
use crate::guard::on_large_stack;
use crate::pool::spawn_promise;
use crate::stores::NativeStateStore;
use flyrail::{
    Index, Journal, Preparation, Receipt, ResourceRef, StateStore, StoreError, ValueError, decode,
    encode,
};
use napi::bindgen_prelude::{Object, PromiseRaw, Uint8Array, Unknown};
use napi::{Env, JsValue};
use napi_derive::napi;
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const BLOCK_STEP: Duration = Duration::from_millis(10);

#[expect(clippy::panic)]
fn internal_panic_work() -> BindingResult<()> {
    panic!("flyrail internal panic")
}

#[napi(
    js_name = "internalPanic",
    catch_unwind,
    ts_return_type = "NativeResult<never>"
)]
pub fn internal_panic(env: &Env) -> napi::Result<Object<'_>> {
    tagged(env, on_large_stack(internal_panic_work))
}

#[napi(
    js_name = "internalPanicAsync",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<never>>"
)]
pub fn internal_panic_async(env: &Env) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    spawn_promise(env, None, internal_panic_work, |_, ()| Ok(()))
}

fn block(duration: Duration, token: Option<&flyrail::Cancellation>) -> BindingResult<()> {
    let started = Instant::now();
    while started.elapsed() < duration {
        if token.is_some_and(flyrail::Cancellation::is_cancelled) {
            return Err(BindingError::Aborted);
        }
        thread::sleep(BLOCK_STEP.min(duration.saturating_sub(started.elapsed())));
    }
    Ok(())
}

#[napi(
    js_name = "internalBlock",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<undefined>>"
)]
pub fn internal_block<'env>(
    env: &'env Env,
    ms: f64,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let token = token.map(|token| token.inner.clone());
    let cancellation = token.clone();
    spawn_promise(
        env,
        token,
        move || {
            let duration = duration_ms(ms)?;
            run_cancellable(cancellation.as_ref(), || {
                block(duration, cancellation.as_ref())
            })
        },
        |_, ()| Ok(()),
    )
}

#[napi(
    js_name = "internalCancelProbe",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<undefined>>"
)]
pub fn internal_cancel_probe<'env>(
    env: &'env Env,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let token = token.map(|token| token.inner.clone());
    spawn_promise(env, token, || Ok(()), |_, ()| Ok(()))
}

#[derive(Default)]
struct Gate {
    generation: u64,
    waiting: u32,
}

struct Rendezvous {
    gate: Mutex<Gate>,
    arrived: Condvar,
}

static RENDEZVOUS: Rendezvous = Rendezvous {
    gate: Mutex::new(Gate {
        generation: 0,
        waiting: 0,
    }),
    arrived: Condvar::new(),
};

fn rendezvous(count: u32, timeout: Duration) -> bool {
    let started = Instant::now();
    let mut gate = RENDEZVOUS
        .gate
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let generation = gate.generation;
    gate.waiting += 1;
    if gate.waiting >= count {
        gate.generation += 1;
        gate.waiting = 0;
        RENDEZVOUS.arrived.notify_all();
        return true;
    }
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            gate.waiting -= 1;
            return false;
        }
        gate = RENDEZVOUS
            .arrived
            .wait_timeout(gate, remaining)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
        if gate.generation != generation {
            return true;
        }
    }
}

#[napi(
    js_name = "internalRendezvous",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<boolean>>"
)]
pub fn internal_rendezvous(
    env: &Env,
    count: u32,
    timeout_ms: f64,
) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    spawn_promise(
        env,
        None,
        move || Ok(rendezvous(count, duration_ms(timeout_ms)?)),
        |_, joined| Ok(joined),
    )
}

#[napi(
    js_name = "internalEchoValue",
    catch_unwind,
    ts_args_type = "value: NativeValue",
    ts_return_type = "NativeResult<NativeValue>"
)]
pub fn internal_echo_value<'env>(env: &'env Env, value: Unknown<'_>) -> napi::Result<Object<'env>> {
    tagged(env, ValueTree::read(env.raw(), value.raw()))
}

type Hold = (mpsc::Sender<()>, thread::JoinHandle<()>);

#[napi]
pub struct NativeLockHold {
    hold: Mutex<Option<Hold>>,
}

#[napi]
impl NativeLockHold {
    #[napi(catch_unwind)]
    pub fn release(&self) {
        let hold = self
            .hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some((release, holder)) = hold {
            let _ = release.send(());
            let _ = holder.join();
        }
    }
}

fn hold_lock(store: Arc<dyn StateStore>, name: String) -> BindingResult<NativeLockHold> {
    let (acquired_tx, acquired_rx) = mpsc::channel::<Result<(), StoreError>>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = thread::Builder::new()
        .name(String::from("flyrail-lock-hold"))
        .spawn(move || match store.lock(&name, Duration::ZERO) {
            Ok(lock) => {
                let _ = acquired_tx.send(Ok(()));
                let _ = release_rx.recv();
                drop(lock);
            }
            Err(error) => {
                let _ = acquired_tx.send(Err(error));
            }
        })
        .map_err(|error| BindingError::Native(format!("could not start a lock holder: {error}")))?;
    match acquired_rx.recv() {
        Ok(Ok(())) => Ok(NativeLockHold {
            hold: Mutex::new(Some((release_tx, holder))),
        }),
        Ok(Err(error)) => {
            let _ = holder.join();
            Err(error.into())
        }
        Err(_) => {
            let _ = holder.join();
            Err(BindingError::Native(String::from(
                "the lock holder stopped before acquiring the lock",
            )))
        }
    }
}

#[napi(
    js_name = "internalHoldLock",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeLockHold>>"
)]
pub fn internal_hold_lock<'env>(
    env: &'env Env,
    store: &NativeStateStore,
    name: String,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let store = Arc::clone(&store.inner);
    spawn_promise(
        env,
        None,
        move || hold_lock(store, name),
        |_, hold| Ok(hold),
    )
}

fn canonical_state(state_type: &str, input: &[u8]) -> BindingResult<Vec<u8>> {
    let canonical = match state_type {
        "Index" => decode::<Index>(input).map(|record| encode(&record)),
        "Journal" => decode::<Journal>(input).map(|record| encode(&record)),
        "Preparation" => decode::<Preparation>(input).map(|record| encode(&record)),
        "Receipt" => decode::<Receipt>(input).map(|record| encode(&record)),
        "ResourceRef" => decode::<ResourceRef>(input).map(|record| encode(&record)),
        other => {
            return Err(BindingError::Argument(format!(
                "unsupported state type: {other}"
            )));
        }
    };
    canonical.map_err(|error: ValueError| error.into())
}

#[napi(
    js_name = "internalCanonicalState",
    catch_unwind,
    ts_return_type = "NativeResult<Uint8Array>"
)]
pub fn internal_canonical_state(
    env: &Env,
    state_type: String,
    bytes: Uint8Array,
) -> napi::Result<Object<'_>> {
    let input = bytes.to_vec();
    tagged(
        env,
        on_large_stack(move || canonical_state(&state_type, &input).map(Uint8Array::from)),
    )
}
