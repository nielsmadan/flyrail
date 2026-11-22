use crate::errors::{BindingError, BindingResult};
use flyrail::{ArrayValue, Date, DateTime, ObjectValue, Scalar, Time, Value};
use napi::bindgen_prelude::{FromNapiValue, ToNapiValue, Uint16Array};
use napi::{check_status, sys};
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::time::Duration;

pub(crate) const MAX_DEPTH: usize = 512;

pub(crate) struct ValueTree(pub Value);

const TOO_DEEP: &str = "value nesting is too deep";

fn argument(message: &str) -> BindingError {
    BindingError::Argument(message.to_owned())
}

fn status<T>(result: napi::Result<T>) -> BindingResult<T> {
    result.map_err(|error| BindingError::Native(error.reason))
}

unsafe fn property(
    env: sys::napi_env,
    object: sys::napi_value,
    name: &str,
) -> BindingResult<sys::napi_value> {
    let key = CString::new(name).map_err(|_| argument("property name contains NUL"))?;
    let mut value = ptr::null_mut();
    status(check_status!(
        unsafe { sys::napi_get_named_property(env, object, key.as_ptr(), &raw mut value) },
        "could not read property {name}"
    ))?;
    Ok(value)
}

unsafe fn element(
    env: sys::napi_env,
    array: sys::napi_value,
    index: u32,
) -> BindingResult<sys::napi_value> {
    let mut value = ptr::null_mut();
    status(check_status!(
        unsafe { sys::napi_get_element(env, array, index, &raw mut value) },
        "could not read element {index}"
    ))?;
    Ok(value)
}

unsafe fn array_length(env: sys::napi_env, array: sys::napi_value) -> BindingResult<u32> {
    let mut length = 0_u32;
    status(check_status!(
        unsafe { sys::napi_get_array_length(env, array, &raw mut length) },
        "could not read array length"
    ))?;
    Ok(length)
}

unsafe fn read<T: FromNapiValue>(env: sys::napi_env, value: sys::napi_value) -> BindingResult<T> {
    status(unsafe { T::from_napi_value(env, value) })
}

unsafe fn read_integer(env: sys::napi_env, value: sys::napi_value) -> BindingResult<i64> {
    let mut integer = 0_i64;
    let mut lossless = false;
    status(check_status!(
        unsafe {
            sys::napi_get_value_bigint_int64(env, value, &raw mut integer, &raw mut lossless)
        },
        "integer values must be bigint"
    ))?;
    if lossless {
        Ok(integer)
    } else {
        Err(argument("integers must fit signed 64 bits"))
    }
}

#[expect(clippy::cast_possible_truncation)]
pub(crate) fn whole(number: f64, label: &str) -> BindingResult<i64> {
    if number.is_finite() && number.fract() == 0.0 {
        Ok(number.clamp(-1e15, 1e15) as i64)
    } else {
        Err(BindingError::Argument(format!(
            "{label} must be an integer"
        )))
    }
}

pub(crate) fn date_of(year: f64, month: f64, day: f64) -> BindingResult<Date> {
    let year = whole(year, "date year")?;
    let month = whole(month, "date month")?;
    let day = whole(day, "date day")?;
    Ok(Date::new(
        u16::try_from(year).unwrap_or(u16::MAX),
        u8::try_from(month).unwrap_or(u8::MAX),
        u8::try_from(day).unwrap_or(u8::MAX),
    )?)
}

pub(crate) fn time_of(
    hour: f64,
    minute: f64,
    second: f64,
    microsecond: f64,
) -> BindingResult<Time> {
    let hour = whole(hour, "time hour")?;
    let minute = whole(minute, "time minute")?;
    let second = whole(second, "time second")?;
    let microsecond = whole(microsecond, "time microsecond")?;
    Ok(Time::new(
        u8::try_from(hour).unwrap_or(u8::MAX),
        u8::try_from(minute).unwrap_or(u8::MAX),
        u8::try_from(second).unwrap_or(u8::MAX),
        u32::try_from(microsecond).unwrap_or(u32::MAX),
    )?)
}

unsafe fn read_date_parts(env: sys::napi_env, object: sys::napi_value) -> BindingResult<Date> {
    date_of(
        unsafe { read(env, property(env, object, "year")?)? },
        unsafe { read(env, property(env, object, "month")?)? },
        unsafe { read(env, property(env, object, "day")?)? },
    )
}

unsafe fn read_time_parts(env: sys::napi_env, object: sys::napi_value) -> BindingResult<Time> {
    time_of(
        unsafe { read(env, property(env, object, "hour")?)? },
        unsafe { read(env, property(env, object, "minute")?)? },
        unsafe { read(env, property(env, object, "second")?)? },
        unsafe { read(env, property(env, object, "microsecond")?)? },
    )
}

enum Node {
    Leaf(Value),
    Container(Frame),
}

enum Frame {
    Array {
        source: sys::napi_value,
        length: u32,
        next: u32,
        items: Vec<Value>,
    },
    Object {
        source: sys::napi_value,
        length: u32,
        next: u32,
        key: String,
        members: Vec<(String, Value)>,
    },
}

impl Frame {
    unsafe fn advance(&mut self, env: sys::napi_env) -> BindingResult<Option<sys::napi_value>> {
        match self {
            Self::Array {
                source,
                length,
                next,
                ..
            } => {
                if *next == *length {
                    return Ok(None);
                }
                let child = unsafe { element(env, *source, *next)? };
                *next += 1;
                Ok(Some(child))
            }
            Self::Object {
                source,
                length,
                next,
                key,
                ..
            } => {
                if *next == *length {
                    return Ok(None);
                }
                let pair = unsafe { element(env, *source, *next)? };
                *next += 1;
                *key = unsafe { read(env, element(env, pair, 0)?)? };
                Ok(Some(unsafe { element(env, pair, 1)? }))
            }
        }
    }

    fn accept(&mut self, value: Value) {
        match self {
            Self::Array { items, .. } => items.push(value),
            Self::Object { key, members, .. } => members.push((std::mem::take(key), value)),
        }
    }

    fn finish(self) -> BindingResult<Value> {
        match self {
            Self::Array { items, .. } => Ok(Value::Array(ArrayValue::new(items))),
            Self::Object { members, .. } => Ok(Value::Object(ObjectValue::new(members)?)),
        }
    }
}

unsafe fn read_node(env: sys::napi_env, wire: sys::napi_value) -> BindingResult<Node> {
    let tag: String = unsafe { read(env, property(env, wire, "t")?)? };
    let leaf = |scalar: Scalar| Ok(Node::Leaf(Value::Scalar(scalar)));
    match tag.as_str() {
        "null" => leaf(Scalar::null()),
        "bool" => leaf(Scalar::boolean(unsafe {
            read(env, property(env, wire, "v")?)?
        })),
        "int" => leaf(Scalar::integer(unsafe {
            read_integer(env, property(env, wire, "v")?)?
        })),
        "float" => leaf(Scalar::float(unsafe {
            read(env, property(env, wire, "v")?)?
        })?),
        "text" => leaf(Scalar::text(unsafe {
            read::<String>(env, property(env, wire, "v")?)?
        })),
        "date" => leaf(Scalar::date(unsafe { read_date_parts(env, wire)? })),
        "time" => leaf(Scalar::time(unsafe { read_time_parts(env, wire)? })),
        "datetime" => {
            let date = unsafe { read_date_parts(env, property(env, wire, "date")?)? };
            let time = unsafe { read_time_parts(env, property(env, wire, "time")?)? };
            let utc: bool = unsafe { read(env, property(env, wire, "utc")?)? };
            leaf(Scalar::datetime(if utc {
                DateTime::coordinated(date, time)
            } else {
                DateTime::local(date, time)
            }))
        }
        "array" => {
            let source = unsafe { property(env, wire, "items")? };
            Ok(Node::Container(Frame::Array {
                source,
                length: unsafe { array_length(env, source)? },
                next: 0,
                items: Vec::new(),
            }))
        }
        "object" => {
            let source = unsafe { property(env, wire, "members")? };
            Ok(Node::Container(Frame::Object {
                source,
                length: unsafe { array_length(env, source)? },
                next: 0,
                key: String::new(),
                members: Vec::new(),
            }))
        }
        _ => Err(argument("unknown value tag")),
    }
}

impl ValueTree {
    pub(crate) fn read(env: sys::napi_env, wire: sys::napi_value) -> BindingResult<Self> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut pending = Some(wire);
        let mut finished: Option<Value> = None;
        loop {
            if let Some(wire) = pending.take() {
                match unsafe { read_node(env, wire)? } {
                    Node::Leaf(value) => finished = Some(value),
                    Node::Container(frame) => {
                        within_depth(stack.len() + 1)?;
                        stack.push(frame);
                    }
                }
            }
            if let Some(value) = finished.take() {
                match stack.last_mut() {
                    None => return Ok(Self(value)),
                    Some(frame) => frame.accept(value),
                }
            }
            let Some(frame) = stack.last_mut() else {
                return Err(argument("value conversion lost its root"));
            };
            match unsafe { frame.advance(env)? } {
                Some(child) => pending = Some(child),
                None => {
                    finished = stack.pop().map(Frame::finish).transpose()?;
                }
            }
        }
    }
}

enum Build<'a> {
    Array {
        wire: sys::napi_value,
        items: &'a [Value],
        list: sys::napi_value,
        next: usize,
    },
    Object {
        wire: sys::napi_value,
        members: &'a [(String, Value)],
        list: sys::napi_value,
        pair: sys::napi_value,
        next: usize,
    },
}

fn set<V: ToNapiValue>(
    env: sys::napi_env,
    object: sys::napi_value,
    name: &str,
    value: V,
) -> napi::Result<()> {
    napi::bindgen_prelude::Object::from_raw(env, object).set(name, value)
}

unsafe fn set_element(
    env: sys::napi_env,
    array: sys::napi_value,
    index: usize,
    value: sys::napi_value,
) -> napi::Result<()> {
    let index = u32::try_from(index).map_err(|_| napi::Error::from_reason("array too long"))?;
    check_status!(
        unsafe { sys::napi_set_element(env, array, index, value) },
        "could not set element {index}"
    )
}

fn new_object(env: sys::napi_env) -> napi::Result<sys::napi_value> {
    let mut value = ptr::null_mut();
    check_status!(
        unsafe { sys::napi_create_object(env, &raw mut value) },
        "could not create object"
    )?;
    Ok(value)
}

fn new_array(env: sys::napi_env, length: usize) -> napi::Result<sys::napi_value> {
    let mut value = ptr::null_mut();
    check_status!(
        unsafe { sys::napi_create_array_with_length(env, length, &raw mut value) },
        "could not create array"
    )?;
    Ok(value)
}

fn set_date(env: sys::napi_env, wire: sys::napi_value, date: Date) -> napi::Result<()> {
    set(env, wire, "year", u32::from(date.year()))?;
    set(env, wire, "month", u32::from(date.month()))?;
    set(env, wire, "day", u32::from(date.day()))
}

fn set_time(env: sys::napi_env, wire: sys::napi_value, time: Time) -> napi::Result<()> {
    set(env, wire, "hour", u32::from(time.hour()))?;
    set(env, wire, "minute", u32::from(time.minute()))?;
    set(env, wire, "second", u32::from(time.second()))?;
    set(env, wire, "microsecond", time.microsecond())
}

fn date_wire(env: sys::napi_env, date: Date) -> napi::Result<sys::napi_value> {
    let wire = new_object(env)?;
    set_date(env, wire, date)?;
    Ok(wire)
}

fn time_wire(env: sys::napi_env, time: Time) -> napi::Result<sys::napi_value> {
    let wire = new_object(env)?;
    set_time(env, wire, time)?;
    Ok(wire)
}

fn scalar_wire(env: sys::napi_env, scalar: &Scalar) -> napi::Result<sys::napi_value> {
    let wire = new_object(env)?;
    if scalar.is_null() {
        set(env, wire, "t", "null")?;
    } else if let Some(value) = scalar.as_boolean() {
        set(env, wire, "t", "bool")?;
        set(env, wire, "v", value)?;
    } else if let Some(value) = scalar.as_integer() {
        set(env, wire, "t", "int")?;
        set(env, wire, "v", napi::bindgen_prelude::BigInt::from(value))?;
    } else if let Some(value) = scalar.as_float() {
        set(env, wire, "t", "float")?;
        set(env, wire, "v", value)?;
    } else if let Some(value) = scalar.as_text() {
        set(env, wire, "t", "text")?;
        set(env, wire, "v", value)?;
    } else if let Some(date) = scalar.as_date() {
        set(env, wire, "t", "date")?;
        set_date(env, wire, date)?;
    } else if let Some(time) = scalar.as_time() {
        set(env, wire, "t", "time")?;
        set_time(env, wire, time)?;
    } else if let Some(datetime) = scalar.as_datetime() {
        set(env, wire, "t", "datetime")?;
        set(env, wire, "date", date_wire(env, datetime.date())?)?;
        set(env, wire, "time", time_wire(env, datetime.time())?)?;
        set(env, wire, "utc", datetime.is_coordinated())?;
    }
    Ok(wire)
}

fn open_container(env: sys::napi_env, value: &Value) -> napi::Result<Option<Build<'_>>> {
    match value {
        Value::Scalar(_) => Ok(None),
        Value::Array(array) => {
            let wire = new_object(env)?;
            let list = new_array(env, array.items().len())?;
            set(env, wire, "t", "array")?;
            set(env, wire, "items", list)?;
            Ok(Some(Build::Array {
                wire,
                items: array.items(),
                list,
                next: 0,
            }))
        }
        Value::Object(object) => {
            let wire = new_object(env)?;
            let list = new_array(env, object.items().len())?;
            set(env, wire, "t", "object")?;
            set(env, wire, "members", list)?;
            Ok(Some(Build::Object {
                wire,
                members: object.items(),
                list,
                pair: ptr::null_mut(),
                next: 0,
            }))
        }
    }
}

impl<'a> Build<'a> {
    fn advance(&mut self, env: sys::napi_env) -> napi::Result<Option<&'a Value>> {
        match self {
            Self::Array { items, next, .. } => {
                let child = items.get(*next);
                *next += 1;
                Ok(child)
            }
            Self::Object {
                members,
                pair,
                next,
                ..
            } => {
                let Some((key, child)) = members.get(*next) else {
                    return Ok(None);
                };
                *next += 1;
                *pair = new_array(env, 2)?;
                unsafe {
                    set_element(
                        env,
                        *pair,
                        0,
                        ToNapiValue::to_napi_value(env, key.as_str())?,
                    )?;
                }
                Ok(Some(child))
            }
        }
    }

    fn attach(&self, env: sys::napi_env, child: sys::napi_value) -> napi::Result<()> {
        match self {
            Self::Array { list, next, .. } => unsafe {
                set_element(env, *list, next.saturating_sub(1), child)
            },
            Self::Object {
                list, pair, next, ..
            } => unsafe {
                set_element(env, *pair, 1, child)?;
                set_element(env, *list, next.saturating_sub(1), *pair)
            },
        }
    }

    const fn wire(&self) -> sys::napi_value {
        match self {
            Self::Array { wire, .. } | Self::Object { wire, .. } => *wire,
        }
    }
}

impl ToNapiValue for ValueTree {
    unsafe fn to_napi_value(env: sys::napi_env, tree: Self) -> napi::Result<sys::napi_value> {
        let mut stack: Vec<Build<'_>> = Vec::new();
        let mut pending: Option<&Value> = Some(&tree.0);
        let mut finished: Option<sys::napi_value> = None;
        loop {
            if let Some(value) = pending.take() {
                match open_container(env, value)? {
                    None => {
                        if let Value::Scalar(scalar) = value {
                            finished = Some(scalar_wire(env, scalar)?);
                        }
                    }
                    Some(frame) => {
                        if stack.len() + 1 > MAX_DEPTH {
                            return Err(napi::Error::new(napi::Status::InvalidArg, TOO_DEEP));
                        }
                        stack.push(frame);
                    }
                }
            }
            if let Some(wire) = finished.take() {
                match stack.last() {
                    None => return Ok(wire),
                    Some(frame) => frame.attach(env, wire)?,
                }
            }
            let Some(frame) = stack.last_mut() else {
                return Err(napi::Error::from_reason("value conversion lost its root"));
            };
            match frame.advance(env)? {
                Some(child) => pending = Some(child),
                None => finished = stack.pop().as_ref().map(Build::wire),
            }
        }
    }
}

pub(crate) fn within_depth(depth: usize) -> BindingResult<()> {
    if depth > MAX_DEPTH {
        Err(BindingError::Argument(String::from(TOO_DEEP)))
    } else {
        Ok(())
    }
}

pub(crate) struct HookUnits(pub Vec<u16>);

impl From<&Uint16Array> for HookUnits {
    fn from(units: &Uint16Array) -> Self {
        Self(units.to_vec())
    }
}

pub(crate) fn absolute_path(text: &str) -> BindingResult<PathBuf> {
    let path = Path::new(text);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Err(BindingError::Argument(String::from(
            "path must be an absolute path",
        )))
    }
}

pub(crate) fn duration_ms(ms: f64) -> BindingResult<Duration> {
    if ms.is_finite() && ms >= 0.0 {
        Duration::try_from_secs_f64(ms / 1000.0)
            .map_err(|error| BindingError::Argument(format!("invalid duration: {error}")))
    } else {
        Err(BindingError::Argument(String::from(
            "duration must be finite and nonnegative",
        )))
    }
}

#[expect(dead_code)]
pub(crate) fn pairs(record: HashMap<String, String>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = record.into_iter().collect();
    pairs.sort_unstable();
    pairs
}
