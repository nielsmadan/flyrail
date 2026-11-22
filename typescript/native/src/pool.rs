use crate::errors::{BindingError, BindingResult, tagged};
use crate::guard::{LARGE_STACK_BYTES, guarded, panic_message};
use flyrail::Cancellation;
use napi::bindgen_prelude::{JsValue, Object, PromiseRaw, ToNapiValue};
use napi::{Env, JsDeferred, sys};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::thread;

const MAX_WORKERS: usize = 64;

enum Dispatch {
    Run,
    Cancelled,
    SpawnFailed(String),
}

struct Job {
    token: Option<Cancellation>,
    run: Box<dyn FnOnce(Dispatch) + Send>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    idle: usize,
    workers: usize,
}

#[derive(Default)]
struct Pool {
    queue: Mutex<Queue>,
    ready: Condvar,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(Pool::default)
}

impl Pool {
    fn submit(&'static self, job: Job) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.jobs.len() >= queue.idle && queue.workers < MAX_WORKERS {
            let spawned = thread::Builder::new()
                .name(format!("flyrail-worker-{}", queue.workers))
                .stack_size(LARGE_STACK_BYTES)
                .spawn(move || self.work());
            match spawned {
                Ok(_) => queue.workers += 1,
                Err(error) => {
                    drop(queue);
                    (job.run)(Dispatch::SpawnFailed(format!(
                        "could not start a flyrail worker: {error}"
                    )));
                    return;
                }
            }
        }
        queue.jobs.push_back(job);
        drop(queue);
        self.ready.notify_one();
    }

    fn next(&self) -> Job {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.idle += 1;
        loop {
            if let Some(job) = queue.jobs.pop_front() {
                queue.idle -= 1;
                return job;
            }
            queue = self
                .ready
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn work(&self) {
        loop {
            let job = self.next();
            let dispatch = if job.token.as_ref().is_some_and(Cancellation::is_cancelled) {
                Dispatch::Cancelled
            } else {
                Dispatch::Run
            };
            let _ = catch_unwind(AssertUnwindSafe(|| (job.run)(dispatch)));
        }
    }
}

struct Settlement<J>(BindingResult<J>);

impl<J: ToNapiValue> ToNapiValue for Settlement<J> {
    unsafe fn to_napi_value(raw: sys::napi_env, settlement: Self) -> napi::Result<sys::napi_value> {
        let env = Env::from_raw(raw);
        let built = catch_unwind(AssertUnwindSafe(|| tagged(&env, settlement.0))).unwrap_or_else(
            |payload| {
                tagged::<()>(
                    &env,
                    Err(BindingError::Panic(panic_message(payload.as_ref()))),
                )
            },
        )?;
        unsafe { Object::to_napi_value(raw, built) }
    }
}

type Resolver<J> = Box<dyn FnOnce(Env) -> napi::Result<Settlement<J>> + Send>;

fn convert<T, J>(
    env: Env,
    outcome: BindingResult<T>,
    to_js: impl FnOnce(Env, T) -> napi::Result<J>,
) -> BindingResult<J> {
    let value = outcome?;
    catch_unwind(AssertUnwindSafe(|| to_js(env, value)))
        .map_err(|payload| BindingError::Panic(panic_message(payload.as_ref())))?
        .map_err(|error| BindingError::Native(error.reason))
}

pub(crate) fn spawn_promise<T, J, W, C>(
    env: &Env,
    token: Option<Cancellation>,
    work: W,
    to_js: C,
) -> napi::Result<PromiseRaw<'_, Object<'static>>>
where
    T: Send + 'static,
    J: ToNapiValue + 'static,
    W: FnOnce() -> BindingResult<T> + Send + 'static,
    C: FnOnce(Env, T) -> napi::Result<J> + Send + 'static,
{
    let (deferred, promise) = env.create_deferred::<Settlement<J>, Resolver<J>>()?;
    let settle = move |deferred: JsDeferred<Settlement<J>, Resolver<J>>,
                       outcome: BindingResult<T>| {
        deferred.resolve(Box::new(move |env| {
            Ok(Settlement(convert(env, outcome, to_js)))
        }));
    };
    let run = move |dispatch: Dispatch| {
        let outcome = match dispatch {
            Dispatch::Run => guarded(work),
            Dispatch::Cancelled => Err(BindingError::Aborted),
            Dispatch::SpawnFailed(message) => Err(BindingError::Native(message)),
        };
        settle(deferred, outcome);
    };
    if token.as_ref().is_some_and(Cancellation::is_cancelled) {
        run(Dispatch::Cancelled);
    } else {
        pool().submit(Job {
            token,
            run: Box::new(run),
        });
    }
    Ok(PromiseRaw::new(env.raw(), promise.raw()))
}

pub(crate) type Writer<T> = for<'env> fn(&'env Env, &T) -> napi::Result<Object<'env>>;

pub(crate) struct Built<T> {
    value: T,
    write: Writer<T>,
}

impl<T> Built<T> {
    pub(crate) const fn new(value: T, write: Writer<T>) -> Self {
        Self { value, write }
    }
}

impl<T> ToNapiValue for Built<T> {
    unsafe fn to_napi_value(raw: sys::napi_env, built: Self) -> napi::Result<sys::napi_value> {
        let env = Env::from_raw(raw);
        let object = (built.write)(&env, &built.value)?;
        unsafe { Object::to_napi_value(raw, object) }
    }
}
