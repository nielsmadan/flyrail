use crate::json::python_str_repr;
use crate::stores::{StateStore, StoreError};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn expect(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn surfaced<T>(result: Result<T, StoreError>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

fn excludes(store: &dyn StateStore, name: &str, timeout: Duration) -> Result<bool, String> {
    match store.lock(name, timeout) {
        Ok(_) => Ok(false),
        Err(StoreError::Busy { .. }) => Ok(true),
        Err(error) => Err(error.to_string()),
    }
}

pub fn check_store(open: impl Fn() -> Arc<dyn StateStore>) -> Result<(), String> {
    let (first, second) = (open(), open());
    let (first, second) = (first.as_ref(), second.as_ref());
    check_records(first, second)?;
    check_key_validation(first)?;
    check_locks(first, second)
}

fn check_records(first: &dyn StateStore, second: &dyn StateStore) -> Result<(), String> {
    expect(
        surfaced(first.read("conformance/absent"))?.is_none(),
        "absent keys must read as None",
    )?;
    expect(
        surfaced(first.write("conformance/a", b"one", None))?,
        "creating an absent key must succeed",
    )?;
    expect(
        surfaced(second.read("conformance/a"))?.as_deref() == Some(b"one"),
        "a write must be visible through another handle",
    )?;
    expect(
        !surfaced(second.write("conformance/a", b"two", None))?,
        "creating an existing key must fail",
    )?;
    expect(
        !surfaced(second.write("conformance/a", b"two", Some(b"other")))?,
        "a stale expected value must fail",
    )?;
    expect(
        surfaced(first.read("conformance/a"))?.as_deref() == Some(b"one"),
        "a failed write must leave the value unchanged",
    )?;
    expect(
        surfaced(second.write("conformance/a", b"two", Some(b"one")))?,
        "a matching expected value must succeed",
    )?;
    expect(
        surfaced(first.write("conformance/empty", b"", None))?,
        "creating an empty value must succeed",
    )?;
    expect(
        surfaced(second.read("conformance/empty"))?.as_deref() == Some(b""),
        "empty values must round-trip",
    )?;
    let binary: Vec<u8> = (0..=u8::MAX).collect();
    expect(
        surfaced(first.write("conformance/binary", &binary, None))?,
        "creating a binary value must succeed",
    )?;
    expect(
        surfaced(second.read("conformance/binary"))?.as_deref() == Some(binary.as_slice()),
        "binary values must round-trip exactly",
    )?;
    expect(
        surfaced(first.write("conformance/ab", b"x", None))?,
        "creating a sibling key must succeed",
    )?;
    expect(
        surfaced(second.keys("conformance/a"))? == ["conformance/a", "conformance/ab"],
        "keys must list prefix matches in sorted order",
    )?;
    expect(
        !surfaced(first.delete("conformance/a", b"one"))?,
        "a stale delete must fail",
    )?;
    expect(
        surfaced(first.delete("conformance/a", b"two"))?,
        "a matching delete must succeed",
    )?;
    expect(
        surfaced(second.read("conformance/a"))?.is_none(),
        "a deleted key must read as None",
    )?;
    expect(
        !surfaced(first.delete("conformance/a", b"two"))?,
        "deleting an absent key must fail",
    )?;
    expect(
        surfaced(second.keys("conformance/"))?
            == ["conformance/ab", "conformance/binary", "conformance/empty"],
        "deleted keys must not be listed",
    )
}

fn check_key_validation(first: &dyn StateStore) -> Result<(), String> {
    for key in ["", "../escape", "Upper/case", "trailing/", "a//b"] {
        match first.read(key) {
            Err(StoreError::InvalidKey { .. }) => {}
            Err(error) => return Err(error.to_string()),
            Ok(_) => {
                return Err(format!(
                    "invalid key {} must raise InvalidKey",
                    python_str_repr(key)
                ));
            }
        }
    }
    Ok(())
}

fn check_locks(first: &dyn StateStore, second: &dyn StateStore) -> Result<(), String> {
    let held = surfaced(first.lock("conformance/lock", Duration::ZERO))?;
    expect(
        excludes(second, "conformance/lock", Duration::ZERO)?,
        "a held lock must exclude another handle",
    )?;
    let started = Instant::now();
    expect(
        excludes(second, "conformance/lock", Duration::from_millis(200))?,
        "a held lock must exclude a bounded wait",
    )?;
    expect(
        started.elapsed() >= Duration::from_millis(150),
        "a bounded lock wait must wait before failing",
    )?;
    expect(
        !excludes(second, "conformance/other", Duration::ZERO)?,
        "different lock names must not exclude",
    )?;
    drop(held);
    expect(
        !excludes(second, "conformance/lock", Duration::ZERO)?,
        "a released lock must be acquirable",
    )
}
