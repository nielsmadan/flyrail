use crate::errors::{BindingError, BindingResult};
use flyrail::{Cancellation, cancellable};
use napi_derive::napi;

#[napi]
pub struct CancelToken {
    pub(crate) inner: Cancellation,
}

#[napi]
impl CancelToken {
    #[napi(constructor, catch_unwind)]
    pub fn new() -> Self {
        Self {
            inner: Cancellation::new(),
        }
    }

    #[napi(catch_unwind)]
    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn run_cancellable<T>(
    token: Option<&Cancellation>,
    work: impl FnOnce() -> BindingResult<T>,
) -> BindingResult<T> {
    match token {
        None => work(),
        Some(token) if token.is_cancelled() => Err(BindingError::Aborted),
        Some(token) => cancellable(token, work).unwrap_or(Err(BindingError::Aborted)),
    }
}
