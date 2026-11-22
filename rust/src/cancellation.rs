#![allow(clippy::redundant_pub_crate)]

use std::cell::RefCell;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct Cancellation {
    requested: Arc<AtomicBool>,
}

impl Cancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.requested.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("operation cancelled before commit")
    }
}

impl std::error::Error for Cancelled {}

struct Scope {
    cancellation: Cancellation,
    committed: bool,
    observed: bool,
}

thread_local! {
    static SCOPES: RefCell<Vec<Scope>> = const { RefCell::new(Vec::new()) };
}

fn with_scopes<R>(operation: impl FnOnce(&mut Vec<Scope>) -> R) -> Option<R> {
    SCOPES
        .try_with(|scopes| {
            scopes
                .try_borrow_mut()
                .ok()
                .map(|mut scopes| operation(&mut scopes))
        })
        .ok()
        .flatten()
}

struct ScopeGuard {
    depth: Option<usize>,
}

impl ScopeGuard {
    fn enter(cancellation: &Cancellation) -> Self {
        let depth = with_scopes(|scopes| {
            scopes.push(Scope {
                cancellation: cancellation.clone(),
                committed: false,
                observed: false,
            });
            scopes.len()
        });
        Self { depth }
    }

    fn exit(mut self) -> bool {
        self.depth
            .take()
            .and_then(leave)
            .is_some_and(|scope| scope.observed)
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        if let Some(depth) = self.depth.take() {
            leave(depth);
        }
    }
}

fn leave(depth: usize) -> Option<Scope> {
    with_scopes(|scopes| {
        if scopes.len() < depth {
            return None;
        }
        scopes.truncate(depth);
        scopes.pop()
    })
    .flatten()
}

pub fn cancellable<R>(
    cancellation: &Cancellation,
    operation: impl FnOnce() -> R,
) -> Result<R, Cancelled> {
    let guard = ScopeGuard::enter(cancellation);
    let value = operation();
    if guard.exit() {
        Err(Cancelled)
    } else {
        Ok(value)
    }
}

pub(crate) fn active() -> bool {
    with_scopes(|scopes| scopes.last().is_some_and(|scope| !scope.committed)).unwrap_or(false)
}

pub(crate) fn observe_interruption() -> bool {
    with_scopes(|scopes| {
        let mut cancelled = false;
        for scope in scopes.iter_mut().filter(|scope| !scope.committed) {
            cancelled = cancelled || scope.cancellation.is_cancelled();
            scope.observed = scope.observed || cancelled;
        }
        cancelled
    })
    .unwrap_or(false)
}

pub(crate) fn begin_commit() -> bool {
    !observe_interruption()
}

pub(crate) fn mark_committed() {
    with_scopes(|scopes| {
        for scope in scopes.iter_mut() {
            scope.committed = true;
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn an_untouched_token_returns_the_value() {
        let token = Cancellation::new();
        assert_eq!(cancellable(&token, || 7), Ok(7));
    }

    #[test]
    fn nothing_is_interrupted_outside_a_scope() {
        assert!(!active());
        assert!(!observe_interruption());
        assert!(begin_commit());
    }

    #[test]
    fn a_cancelled_token_vetoes_the_commit_and_reports_cancelled() {
        let token = Cancellation::new();
        token.cancel();
        let granted = Cell::new(true);
        let result = cancellable(&token, || granted.set(begin_commit()));
        assert!(!granted.get());
        assert_eq!(result, Err(Cancelled));
        assert_eq!(Cancelled.to_string(), "operation cancelled before commit");
    }

    #[test]
    fn a_granted_commit_leaves_the_scope_cancellable() {
        let token = Cancellation::new();
        let seen = Cell::new((false, false));
        let result = cancellable(&token, || {
            let granted = begin_commit();
            token.cancel();
            seen.set((granted, observe_interruption()));
        });
        assert_eq!(seen.get(), (true, true));
        assert_eq!(result, Err(Cancelled));
    }

    #[test]
    fn a_committed_scope_ignores_cancellation() {
        let token = Cancellation::new();
        let result = cancellable(&token, || {
            mark_committed();
            token.cancel();
            (active(), observe_interruption(), begin_commit())
        });
        assert_eq!(result, Ok((false, false, true)));
    }

    #[test]
    fn an_enclosing_token_interrupts_nested_scopes() {
        let outer = Cancellation::new();
        let seen = Cell::new(None);
        let result = cancellable(&outer, || {
            let nested = cancellable(&Cancellation::new(), || {
                outer.cancel();
                observe_interruption()
            });
            seen.set(Some(nested));
        });
        assert_eq!(seen.get(), Some(Err(Cancelled)));
        assert_eq!(result, Err(Cancelled));
    }

    #[test]
    fn a_nested_commit_makes_the_enclosing_scope_uninterruptible() {
        let outer = Cancellation::new();
        let result = cancellable(&outer, || {
            let nested = cancellable(&Cancellation::new(), mark_committed);
            outer.cancel();
            (nested, active(), observe_interruption())
        });
        assert_eq!(result, Ok((Ok(()), false, false)));
    }

    #[test]
    fn the_enclosing_scope_is_restored_after_a_nested_scope_returns() {
        let outer = Cancellation::new();
        let seen = Cell::new((false, false, false));
        let result = cancellable(&outer, || {
            let nested = cancellable(&Cancellation::new(), active);
            let restored = active();
            outer.cancel();
            seen.set((nested == Ok(true), restored, observe_interruption()));
        });
        assert_eq!(seen.get(), (true, true, true));
        assert_eq!(result, Err(Cancelled));
    }

    #[test]
    fn a_cancelled_inner_token_does_not_cancel_a_scope_that_commits_afterwards() {
        let outer = Cancellation::new();
        let inner = Cancellation::new();
        let result = cancellable(&outer, || {
            inner.cancel();
            let nested = cancellable(&inner, observe_interruption);
            mark_committed();
            nested
        });
        assert_eq!(result, Ok(Err(Cancelled)));
    }

    #[test]
    fn a_panicking_operation_leaves_no_scope_installed() {
        let token = Cancellation::new();
        let outcome = std::panic::catch_unwind(|| {
            cancellable(&token, || -> u8 { panic!("operation failed") })
        });
        assert!(outcome.is_err());
        assert!(!active());
        token.cancel();
        assert!(!observe_interruption());
        assert_eq!(cancellable(&Cancellation::new(), || 1), Ok(1));
    }
}
