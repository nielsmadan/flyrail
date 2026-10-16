use std::path::Path;

#[cfg_attr(not(test), allow(dead_code))]
pub enum Seam<'a> {
    Boundary(&'static str),
    Rename {
        source: &'a Path,
        destination: &'a Path,
        done: bool,
    },
    Unlink(&'a Path),
}

#[cfg(test)]
pub type Hook = Box<dyn FnMut(&Seam<'_>)>;

#[cfg(test)]
thread_local! {
    static HOOK: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn set_hook(hook: Option<Hook>) {
    HOOK.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
pub fn fire(seam: &Seam<'_>) {
    let Some(mut hook) = HOOK.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    hook(seam);
    HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(hook);
        }
    });
}

#[cfg(not(test))]
pub const fn fire(_seam: &Seam<'_>) {}
