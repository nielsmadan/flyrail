use crate::convert::{ValueTree, date_of, time_of, whole};
use crate::errors::{BindingResult, tagged};
use crate::guard::{guarded, on_large_stack};
use flyrail::{Date, DateTime, Time, Value};
use napi::bindgen_prelude::{Object, Uint8Array, Unknown};
use napi::{Env, JsValue};
use napi_derive::napi;

#[napi(object)]
pub struct JsDate {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub text: String,
}

#[napi(object)]
pub struct JsTime {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub microsecond: u32,
    pub text: String,
}

#[napi(object)]
pub struct JsDateTime {
    pub date: JsDate,
    pub time: JsTime,
    pub utc: bool,
    pub text: String,
}

#[napi(object)]
pub struct DateParts {
    pub year: f64,
    pub month: f64,
    pub day: f64,
}

#[napi(object)]
pub struct TimeParts {
    pub hour: f64,
    pub minute: f64,
    pub second: f64,
    pub microsecond: f64,
}

fn date_view(date: Date) -> JsDate {
    JsDate {
        year: u32::from(date.year()),
        month: u32::from(date.month()),
        day: u32::from(date.day()),
        text: date.to_string(),
    }
}

fn time_view(time: Time) -> JsTime {
    JsTime {
        hour: u32::from(time.hour()),
        minute: u32::from(time.minute()),
        second: u32::from(time.second()),
        microsecond: time.microsecond(),
        text: time.to_string(),
    }
}

fn datetime_view(datetime: DateTime) -> JsDateTime {
    JsDateTime {
        date: date_view(datetime.date()),
        time: time_view(datetime.time()),
        utc: datetime.is_coordinated(),
        text: datetime.to_string(),
    }
}

fn parts(date: &DateParts, time: &TimeParts) -> BindingResult<(Date, Time)> {
    Ok((
        date_of(date.year, date.month, date.day)?,
        time_of(time.hour, time.minute, time.second, time.microsecond)?,
    ))
}

#[napi(
    js_name = "temporalDate",
    catch_unwind,
    ts_return_type = "NativeResult<JsDate>"
)]
pub fn temporal_date(env: &Env, year: f64, month: f64, day: f64) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(date_view(date_of(year, month, day)?))))
}

#[napi(
    js_name = "temporalDateParse",
    catch_unwind,
    ts_return_type = "NativeResult<JsDate>"
)]
pub fn temporal_date_parse(env: &Env, text: String) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(date_view(text.parse::<Date>()?))))
}

#[napi(
    js_name = "temporalTime",
    catch_unwind,
    ts_return_type = "NativeResult<JsTime>"
)]
pub fn temporal_time(
    env: &Env,
    hour: f64,
    minute: f64,
    second: f64,
    microsecond: f64,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| Ok(time_view(time_of(hour, minute, second, microsecond)?))),
    )
}

#[napi(
    js_name = "temporalTimeParse",
    catch_unwind,
    ts_return_type = "NativeResult<JsTime>"
)]
pub fn temporal_time_parse(env: &Env, text: String) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(time_view(text.parse::<Time>()?))))
}

#[napi(
    js_name = "temporalDateTimeLocal",
    catch_unwind,
    ts_return_type = "NativeResult<JsDateTime>"
)]
pub fn temporal_date_time_local(
    env: &Env,
    date: DateParts,
    time: TimeParts,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let (date, time) = parts(&date, &time)?;
            Ok(datetime_view(DateTime::local(date, time)))
        }),
    )
}

#[napi(
    js_name = "temporalDateTimeUtc",
    catch_unwind,
    ts_return_type = "NativeResult<JsDateTime>"
)]
pub fn temporal_date_time_utc(
    env: &Env,
    date: DateParts,
    time: TimeParts,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let (date, time) = parts(&date, &time)?;
            Ok(datetime_view(DateTime::coordinated(date, time)))
        }),
    )
}

#[napi(
    js_name = "temporalDateTimeWithOffset",
    catch_unwind,
    ts_return_type = "NativeResult<JsDateTime>"
)]
pub fn temporal_date_time_with_offset(
    env: &Env,
    date: DateParts,
    time: TimeParts,
    offset_seconds: f64,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let (date, time) = parts(&date, &time)?;
            let offset = whole(offset_seconds, "datetime offset")?;
            let offset = i32::try_from(offset).unwrap_or(i32::MAX);
            Ok(datetime_view(DateTime::with_offset(date, time, offset)?))
        }),
    )
}

#[napi(
    js_name = "temporalDateTimeParse",
    catch_unwind,
    ts_return_type = "NativeResult<JsDateTime>"
)]
pub fn temporal_date_time_parse(env: &Env, text: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| Ok(datetime_view(text.parse::<DateTime>()?))),
    )
}

fn read_tree(env: &Env, wire: &Unknown<'_>) -> BindingResult<Value> {
    ValueTree::read(env.raw(), wire.raw()).map(|tree| tree.0)
}

#[napi(
    js_name = "valuesEncode",
    catch_unwind,
    ts_args_type = "value: NativeValue",
    ts_return_type = "NativeResult<Uint8Array>"
)]
pub fn values_encode<'env>(env: &'env Env, value: Unknown<'_>) -> napi::Result<Object<'env>> {
    let encoded = read_tree(env, &value)
        .and_then(|value| on_large_stack(move || Ok(Uint8Array::from(value.semantic_bytes()))));
    tagged(env, encoded)
}

#[napi(
    js_name = "valuesDecode",
    catch_unwind,
    ts_return_type = "NativeResult<NativeValue>"
)]
pub fn values_decode(env: &Env, bytes: Uint8Array) -> napi::Result<Object<'_>> {
    let bytes = bytes.to_vec();
    let decoded = on_large_stack(move || Ok(Value::from_semantic_bytes(&bytes)?)).map(ValueTree);
    tagged(env, decoded)
}

#[napi(
    js_name = "valuesEqual",
    catch_unwind,
    ts_args_type = "left: NativeValue, right: NativeValue",
    ts_return_type = "NativeResult<boolean>"
)]
pub fn values_equal<'env>(
    env: &'env Env,
    left: Unknown<'_>,
    right: Unknown<'_>,
) -> napi::Result<Object<'env>> {
    let equal = read_tree(env, &left)
        .and_then(|left| read_tree(env, &right).map(|right| (left, right)))
        .and_then(|(left, right)| on_large_stack(move || Ok(left == right)));
    tagged(env, equal)
}
