use crate::json::Json;
use crate::temporal::{Date, DateTime, Time};
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueError {
    message: String,
    python_type_error: bool,
    nesting: bool,
}

impl ValueError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            python_type_error: false,
            nesting: false,
        }
    }

    pub(crate) fn type_error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            python_type_error: true,
            nesting: false,
        }
    }

    pub(crate) fn nesting(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            python_type_error: false,
            nesting: true,
        }
    }

    pub(crate) const fn is_python_type_error(&self) -> bool {
        self.python_type_error
    }

    pub(crate) const fn is_nesting(&self) -> bool {
        self.nesting
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ValueError {}

#[derive(Clone, Debug)]
pub(crate) enum ScalarKind {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    Text(String),
    Date(Date),
    Time(Time),
    DateTime(DateTime),
}

#[derive(Clone, Debug)]
pub struct Scalar(ScalarKind);

impl Scalar {
    pub const fn null() -> Self {
        Self(ScalarKind::Null)
    }

    pub const fn boolean(value: bool) -> Self {
        Self(ScalarKind::Boolean(value))
    }

    pub const fn integer(value: i64) -> Self {
        Self(ScalarKind::Integer(value))
    }

    pub fn float(value: f64) -> Result<Self, ValueError> {
        if value.is_finite() {
            Ok(Self(ScalarKind::Float(value)))
        } else {
            Err(ValueError::new("floats must be finite"))
        }
    }

    pub(crate) const fn raw_float(value: f64) -> Self {
        Self(ScalarKind::Float(value))
    }

    pub fn text(value: impl Into<String>) -> Self {
        Self(ScalarKind::Text(value.into()))
    }

    pub const fn date(value: Date) -> Self {
        Self(ScalarKind::Date(value))
    }

    pub const fn time(value: Time) -> Self {
        Self(ScalarKind::Time(value))
    }

    pub const fn datetime(value: DateTime) -> Self {
        Self(ScalarKind::DateTime(value))
    }

    pub const fn is_null(&self) -> bool {
        matches!(self.0, ScalarKind::Null)
    }

    pub const fn as_boolean(&self) -> Option<bool> {
        match self.0 {
            ScalarKind::Boolean(value) => Some(value),
            _ => None,
        }
    }

    pub const fn as_integer(&self) -> Option<i64> {
        match self.0 {
            ScalarKind::Integer(value) => Some(value),
            _ => None,
        }
    }

    pub const fn as_float(&self) -> Option<f64> {
        match self.0 {
            ScalarKind::Float(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match &self.0 {
            ScalarKind::Text(value) => Some(value),
            _ => None,
        }
    }

    pub const fn as_date(&self) -> Option<Date> {
        match self.0 {
            ScalarKind::Date(value) => Some(value),
            _ => None,
        }
    }

    pub const fn as_time(&self) -> Option<Time> {
        match self.0 {
            ScalarKind::Time(value) => Some(value),
            _ => None,
        }
    }

    pub const fn as_datetime(&self) -> Option<DateTime> {
        match self.0 {
            ScalarKind::DateTime(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) const fn kind(&self) -> &ScalarKind {
        &self.0
    }
}

impl PartialEq for Scalar {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (ScalarKind::Null, ScalarKind::Null) => true,
            (ScalarKind::Boolean(left), ScalarKind::Boolean(right)) => left == right,
            (ScalarKind::Integer(left), ScalarKind::Integer(right)) => left == right,
            (ScalarKind::Float(left), ScalarKind::Float(right)) => {
                left.to_bits() == right.to_bits()
            }
            (ScalarKind::Text(left), ScalarKind::Text(right)) => left == right,
            (ScalarKind::Date(left), ScalarKind::Date(right)) => left == right,
            (ScalarKind::Time(left), ScalarKind::Time(right)) => left == right,
            (ScalarKind::DateTime(left), ScalarKind::DateTime(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for Scalar {}

impl Hash for Scalar {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(&self.0).hash(state);
        match &self.0 {
            ScalarKind::Null => {}
            ScalarKind::Boolean(value) => value.hash(state),
            ScalarKind::Integer(value) => value.hash(state),
            ScalarKind::Float(value) => value.to_bits().hash(state),
            ScalarKind::Text(value) => value.hash(state),
            ScalarKind::Date(value) => value.hash(state),
            ScalarKind::Time(value) => value.hash(state),
            ScalarKind::DateTime(value) => value.hash(state),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ArrayValue {
    items: Vec<Value>,
}

impl ArrayValue {
    pub fn new(items: impl IntoIterator<Item = Value>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    pub fn items(&self) -> &[Value] {
        &self.items
    }

    pub const fn len(&self) -> usize {
        self.items.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ObjectValue {
    items: Vec<(String, Value)>,
}

impl ObjectValue {
    pub fn new<K: Into<String>>(
        items: impl IntoIterator<Item = (K, Value)>,
    ) -> Result<Self, ValueError> {
        let mut items: Vec<(String, Value)> = items
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect();
        items.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        if items
            .windows(2)
            .any(|pair| pair.first().map(|entry| &entry.0) == pair.get(1).map(|entry| &entry.0))
        {
            return Err(ValueError::new("duplicate object key"));
        }
        Ok(Self { items })
    }

    pub fn items(&self) -> &[(String, Value)] {
        &self.items
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.items
            .binary_search_by(|entry| entry.0.as_bytes().cmp(key.as_bytes()))
            .ok()
            .and_then(|index| self.items.get(index).map(|entry| &entry.1))
    }

    pub(crate) fn from_pairs<K: Into<String>>(items: impl IntoIterator<Item = (K, Value)>) -> Self {
        let mut items: Vec<(String, Value)> = items
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect();
        items.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        Self { items }
    }

    pub const fn len(&self) -> usize {
        self.items.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Value {
    Scalar(Scalar),
    Array(ArrayValue),
    Object(ObjectValue),
}

impl Value {
    pub fn semantic_bytes(&self) -> Vec<u8> {
        let mut encoded = String::new();
        semantic_record(self).encode(&mut encoded);
        encoded.into_bytes()
    }

    pub fn from_semantic_bytes(bytes: &[u8]) -> Result<Self, ValueError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| ValueError::new("semantic values must be UTF-8"))?;
        value_from_record(&Json::parse(text)?)
    }
}

impl From<Scalar> for Value {
    fn from(value: Scalar) -> Self {
        Self::Scalar(value)
    }
}

impl From<ArrayValue> for Value {
    fn from(value: ArrayValue) -> Self {
        Self::Array(value)
    }
}

impl From<ObjectValue> for Value {
    fn from(value: ObjectValue) -> Self {
        Self::Object(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Scalar(Scalar::boolean(value))
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Scalar(Scalar::integer(value))
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Scalar(Scalar::text(value))
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Scalar(Scalar::text(value))
    }
}

fn tagged(tag: &str, payload: Json) -> Json {
    Json::Array(vec![Json::Str(String::from(tag)), payload])
}

pub(crate) fn semantic_record(value: &Value) -> Json {
    match value {
        Value::Object(object) => {
            let members = object
                .items()
                .iter()
                .map(|(key, item)| Json::Array(vec![Json::Str(key.clone()), semantic_record(item)]))
                .collect();
            tagged("object", Json::Array(members))
        }
        Value::Array(array) => {
            let members = array.items().iter().map(semantic_record).collect();
            tagged("array", Json::Array(members))
        }
        Value::Scalar(scalar) => match scalar.kind() {
            ScalarKind::Null => Json::Array(vec![Json::Str(String::from("null"))]),
            ScalarKind::Boolean(item) => tagged("boolean", Json::Bool(*item)),
            ScalarKind::Integer(item) => tagged("integer", Json::Str(item.to_string())),
            ScalarKind::Float(item) => tagged("float", Json::Str(float_hex(*item))),
            ScalarKind::Text(item) => tagged("string", Json::Str(item.clone())),
            ScalarKind::Date(item) => tagged("date", Json::Str(item.to_string())),
            ScalarKind::Time(item) => tagged("time", Json::Str(item.to_string())),
            ScalarKind::DateTime(item) => tagged("datetime", Json::Str(item.to_string())),
        },
    }
}

pub(crate) fn value_from_json(value: &Json) -> Result<Value, ValueError> {
    match value {
        Json::Null => Ok(Value::Scalar(Scalar::null())),
        Json::Bool(item) => Ok(Value::Scalar(Scalar::boolean(*item))),
        Json::Int(item) => Ok(Value::Scalar(Scalar::integer(*item))),
        Json::BigInt(_) => Err(ValueError::new("integers must fit signed 64 bits")),
        Json::Float(item) => Ok(Value::Scalar(Scalar::float(*item)?)),
        Json::Str(item) => Ok(Value::Scalar(Scalar::text(item.clone()))),
        Json::Array(items) => Ok(Value::Array(ArrayValue::new(
            items
                .iter()
                .map(value_from_json)
                .collect::<Result<Vec<_>, _>>()?,
        ))),
        Json::Object(items) => Ok(Value::Object(ObjectValue::new(
            items
                .iter()
                .map(|(key, item)| Ok((key.clone(), value_from_json(item)?)))
                .collect::<Result<Vec<_>, _>>()?,
        )?)),
    }
}

pub(crate) fn value_from_record(record: &Json) -> Result<Value, ValueError> {
    let entries = record
        .as_array()
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| ValueError::new("semantic values must be tagged arrays"))?;
    let kind = entries
        .first()
        .and_then(Json::as_str)
        .ok_or_else(|| ValueError::new("semantic values must be tagged arrays"))?;
    if kind == "null" && entries.len() == 1 {
        return Ok(Value::Scalar(Scalar::null()));
    }
    if entries.len() != 2 {
        return Err(ValueError::new("semantic value has an invalid field count"));
    }
    let raw = entries
        .get(1)
        .ok_or_else(|| ValueError::new("semantic value has an invalid field count"))?;
    match (kind, raw) {
        ("array", Json::Array(members)) => {
            let items = members
                .iter()
                .map(value_from_record)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(ArrayValue::new(items)))
        }
        ("object", Json::Array(members)) => {
            let mut pairs: Vec<(String, Value)> = Vec::with_capacity(members.len());
            for member in members {
                let pair = member
                    .as_array()
                    .filter(|pair| pair.len() == 2)
                    .ok_or_else(|| {
                        ValueError::new("semantic object members must be name/value pairs")
                    })?;
                let name = pair.first().and_then(Json::as_str).ok_or_else(|| {
                    ValueError::new("semantic object members must be name/value pairs")
                })?;
                let item = pair.get(1).ok_or_else(|| {
                    ValueError::new("semantic object members must be name/value pairs")
                })?;
                pairs.push((String::from(name), value_from_record(item)?));
            }
            ObjectValue::new(pairs).map(Value::Object)
        }
        ("boolean", Json::Bool(item)) => Ok(Value::Scalar(Scalar::boolean(*item))),
        (_, Json::Str(payload)) => {
            let scalar = match kind {
                "string" => Scalar::text(payload.clone()),
                "integer" => Scalar::integer(semantic_integer(payload)?),
                "float" => Scalar::float(float_from_hex(payload)?)?,
                "date" => Scalar::date(payload.parse()?),
                "time" => Scalar::time(payload.parse()?),
                "datetime" => Scalar::datetime(payload.parse()?),
                _ => return Err(ValueError::new("unknown semantic scalar kind")),
            };
            let value = Value::Scalar(scalar);
            if semantic_record(&value) == *record {
                Ok(value)
            } else {
                Err(ValueError::new("semantic scalar encoding is not canonical"))
            }
        }
        _ => Err(ValueError::new(
            "semantic scalar payload has the wrong type",
        )),
    }
}

fn semantic_integer(text: &str) -> Result<i64, ValueError> {
    let body = text.strip_prefix('-').unwrap_or(text);
    if body.is_empty() || !body.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ValueError::new(format!(
            "invalid literal for int() with base 10: {}",
            crate::json::python_repr(&Json::Str(String::from(text)))
        )));
    }
    text.parse()
        .map_err(|_| ValueError::new("integers must fit signed 64 bits"))
}

pub(crate) fn float_hex(value: f64) -> String {
    if value == 0.0 {
        return String::from(if value.is_sign_negative() {
            "-0x0.0p+0"
        } else {
            "0x0.0p+0"
        });
    }
    let bits = value.to_bits();
    let sign = if bits >> 63 == 1 { "-" } else { "" };
    let biased = i32::try_from((bits >> 52) & 0x7FF).unwrap_or_default();
    let fraction = bits & 0x000F_FFFF_FFFF_FFFF;
    let (lead, exponent) = if biased == 0 {
        (0, -1022)
    } else {
        (1, biased - 1023)
    };
    format!("{sign}0x{lead:x}.{fraction:013x}p{exponent:+}")
}

pub(crate) fn float_from_hex(text: &str) -> Result<f64, ValueError> {
    let malformed = || ValueError::new("float hexadecimal literal is malformed");
    let negative = text.starts_with('-');
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    let body = body
        .strip_prefix("0x")
        .or_else(|| body.strip_prefix("0X"))
        .ok_or_else(malformed)?;
    let (digits, exponent_text) = match body.split_once(['p', 'P']) {
        Some((digits, exponent)) => (digits, Some(exponent)),
        None => (body, None),
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (digits, ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return Err(malformed());
    }
    if whole.len() + fraction.len() > 16 {
        return Err(ValueError::new(
            "float hexadecimal literal has too many digits",
        ));
    }
    let mut mantissa: u64 = 0;
    for digit in whole.bytes().chain(fraction.bytes()) {
        let nibble = char::from(digit).to_digit(16).ok_or_else(malformed)?;
        mantissa = mantissa * 16 + u64::from(nibble);
    }
    let exponent = match exponent_text {
        Some(exponent) => {
            let sign = if exponent.starts_with('-') { -1 } else { 1 };
            let magnitude = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
            if magnitude.is_empty() || !magnitude.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(malformed());
            }
            sign * magnitude
                .parse::<i32>()
                .map_err(|_| ValueError::new("float exponent is out of range"))?
        }
        None => 0,
    };
    let scale = i64::from(exponent) - 4 * i64::try_from(fraction.len()).unwrap_or(i64::MAX);
    #[allow(clippy::cast_precision_loss)]
    let value = scale_by_power_of_two(mantissa as f64, scale);
    if !value.is_finite() {
        return Err(ValueError::new("float hexadecimal literal is out of range"));
    }
    Ok(if negative { -value } else { value })
}

fn scale_by_power_of_two(value: f64, exponent: i64) -> f64 {
    const UP: f64 = f64::from_bits(0x7FE0_0000_0000_0000);
    const DOWN: f64 = f64::from_bits(0x0360_0000_0000_0000);
    let mut value = value;
    let mut exponent = exponent.clamp(-4_096, 4_096);
    while exponent > 1_023 {
        value *= UP;
        exponent -= 1_023;
        if !value.is_finite() {
            return value;
        }
    }
    while exponent < -1_022 {
        value *= DOWN;
        exponent += 969;
        if value == 0.0 {
            return value;
        }
    }
    let factor = f64::from_bits(u64::try_from(1_023 + exponent).unwrap_or_default() << 52);
    value * factor
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::{ArrayValue, ObjectValue, Scalar, Value, float_from_hex, float_hex};
    use crate::json::Json;
    use crate::temporal::{Date, DateTime, Time};
    use crate::test_support::fixture;
    use sha2::{Digest as _, Sha256};
    use std::collections::hash_map::DefaultHasher;
    use std::fmt::Write as _;
    use std::hash::{Hash as _, Hasher as _};

    fn encoded(value: &Value) -> String {
        String::from_utf8(value.semantic_bytes()).unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    fn hashed(scalar: &Scalar) -> u64 {
        let mut hasher = DefaultHasher::new();
        scalar.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn encodes_scalars_with_python_tags() {
        assert_eq!(encoded(&Value::Scalar(Scalar::null())), r#"["null"]"#);
        assert_eq!(
            encoded(&Value::Scalar(Scalar::boolean(true))),
            r#"["boolean",true]"#
        );
        assert_eq!(
            encoded(&Value::Scalar(Scalar::integer(-17))),
            r#"["integer","-17"]"#
        );
        assert_eq!(
            encoded(&Value::Scalar(Scalar::float(1.5).unwrap())),
            r#"["float","0x1.8000000000000p+0"]"#
        );
        assert_eq!(encoded(&Value::from("é")), r#"["string","é"]"#);
        assert_eq!(
            encoded(&Value::Scalar(Scalar::date(Date::new(2026, 1, 2).unwrap()))),
            r#"["date","2026-01-02"]"#
        );
        assert_eq!(
            encoded(&Value::Scalar(Scalar::time(Time::new(1, 2, 3, 0).unwrap()))),
            r#"["time","01:02:03.000000"]"#
        );
    }

    #[test]
    fn encodes_offset_datetimes_as_coordinated_time() {
        let value = Value::Scalar(Scalar::datetime(
            DateTime::with_offset(
                Date::new(2026, 1, 2).unwrap(),
                Time::new(5, 2, 3, 0).unwrap(),
                5 * 3_600,
            )
            .unwrap(),
        ));
        assert_eq!(
            encoded(&value),
            r#"["datetime","2026-01-02T00:02:03.000000+00:00"]"#
        );
    }

    #[test]
    fn keeps_object_members_sorted_by_key_bytes() {
        let object = ObjectValue::new([
            ("é", Value::from(1)),
            ("b", Value::from(2)),
            ("a", Value::from(3)),
        ])
        .unwrap();
        let keys: Vec<&str> = object.items().iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["a", "b", "é"]);
        assert_eq!(object.get("b"), Some(&Value::from(2)));
    }

    #[test]
    fn rejects_duplicate_object_keys() {
        let duplicate = ObjectValue::new([("a", Value::from(1)), ("a", Value::from(2))]);
        assert_eq!(duplicate.unwrap_err().message(), "duplicate object key");
    }

    #[test]
    fn rejects_non_finite_floats() {
        assert_eq!(
            Scalar::float(f64::NAN).unwrap_err().message(),
            "floats must be finite"
        );
        assert!(Scalar::float(f64::INFINITY).is_err());
        assert!(Scalar::float(f64::NEG_INFINITY).is_err());
        assert_eq!(
            Scalar::float(f64::MIN_POSITIVE).unwrap().as_float(),
            Some(f64::MIN_POSITIVE)
        );
    }

    #[test]
    fn distinguishes_scalar_types_without_coercion() {
        assert_ne!(Scalar::integer(1), Scalar::boolean(true));
        assert_ne!(Scalar::integer(1), Scalar::float(1.0).unwrap());
        assert_ne!(Scalar::integer(1), Scalar::text("1"));
        assert_ne!(Scalar::float(0.0).unwrap(), Scalar::float(-0.0).unwrap());
        assert_eq!(Scalar::float(0.5).unwrap(), Scalar::float(0.5).unwrap());
        assert_eq!(
            hashed(&Scalar::float(0.5).unwrap()),
            hashed(&Scalar::float(0.5).unwrap())
        );
        assert_ne!(hashed(&Scalar::integer(1)), hashed(&Scalar::text("1")));
    }

    #[test]
    fn treats_equal_instants_with_different_offsets_as_one_value() {
        let utc = Scalar::datetime(DateTime::coordinated(
            Date::new(2026, 1, 1).unwrap(),
            Time::new(0, 0, 0, 0).unwrap(),
        ));
        let shifted = Scalar::datetime(
            DateTime::with_offset(
                Date::new(2026, 1, 1).unwrap(),
                Time::new(1, 0, 0, 0).unwrap(),
                3_600,
            )
            .unwrap(),
        );
        assert_eq!(utc, shifted);
        assert_eq!(hashed(&utc), hashed(&shifted));
        let naive = Scalar::datetime(DateTime::local(
            Date::new(2026, 1, 1).unwrap(),
            Time::new(0, 0, 0, 0).unwrap(),
        ));
        assert_ne!(utc, naive);
    }

    #[test]
    fn treats_object_member_order_as_insignificant() {
        let first = ObjectValue::new([("b", Value::from(2)), ("a", Value::from(1))]).unwrap();
        let second = ObjectValue::new([("a", Value::from(1)), ("b", Value::from(2))]).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn writes_non_ascii_and_delete_characters_literally() {
        assert_eq!(
            encoded(&Value::from("\u{7f}\u{ad}\u{2028}")),
            "[\"string\",\"\u{7f}\u{ad}\u{2028}\"]"
        );
    }

    #[test]
    fn rejects_semantic_bytes_that_are_not_utf8() {
        assert_eq!(
            Value::from_semantic_bytes(&[0xFF, 0xFE])
                .unwrap_err()
                .message(),
            "semantic values must be UTF-8"
        );
    }

    #[test]
    fn writes_python_float_hex_spellings() {
        assert_eq!(float_hex(0.0), "0x0.0p+0");
        assert_eq!(float_hex(-0.0), "-0x0.0p+0");
        assert_eq!(float_hex(1.0), "0x1.0000000000000p+0");
        assert_eq!(float_hex(f64::from_bits(1)), "0x0.0000000000001p-1022");
        assert_eq!(float_hex(f64::MAX), "0x1.fffffffffffffp+1023");
        assert_eq!(float_hex(-2.5), "-0x1.4000000000000p+1");
    }

    #[test]
    fn round_trips_float_hex_across_the_exponent_range() {
        let samples = [
            0.0,
            -0.0,
            1.0,
            -2.5,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::from_bits(1),
            f64::from_bits(0x000F_FFFF_FFFF_FFFF),
            1e-300,
            1e300,
            0.1,
            std::f64::consts::PI,
        ];
        for sample in samples {
            let text = float_hex(sample);
            let parsed = float_from_hex(&text).unwrap();
            assert_eq!(
                parsed.to_bits(),
                sample.to_bits(),
                "round trip failed for {text}"
            );
        }
    }

    #[test]
    fn rejects_non_canonical_scalar_records() {
        for text in [
            r#"["integer","01"]"#,
            r#"["integer","+1"]"#,
            r#"["float","0x1.8p+0"]"#,
            r#"["datetime","2026-01-02T00:02:03+00:00"]"#,
            r#"["datetime","2026-01-02T05:02:03.000000+05:00"]"#,
            r#"["time","01:02:03"]"#,
            r#"["date","2026-1-02"]"#,
            r#"["nope","1"]"#,
            r#"["integer"]"#,
            r#"["null",1]"#,
            r#"["boolean","true"]"#,
            "[]",
            "{}",
        ] {
            assert!(
                Value::from_semantic_bytes(text.as_bytes()).is_err(),
                "must reject {text}"
            );
        }
    }

    #[test]
    fn round_trips_nested_containers() {
        let value = Value::Object(
            ObjectValue::new([
                ("a", Value::from(1)),
                (
                    "b",
                    Value::Array(ArrayValue::new([
                        Value::from(true),
                        Value::Scalar(Scalar::null()),
                    ])),
                ),
            ])
            .unwrap(),
        );
        let bytes = value.semantic_bytes();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"["object",[["a",["integer","1"]],["b",["array",[["boolean",true],["null"]]]]]]"#
        );
        assert_eq!(Value::from_semantic_bytes(&bytes).unwrap(), value);
    }

    #[test]
    fn round_trips_the_encodings_python_produces() {
        let canonical = [
            r#"["null"]"#,
            r#"["boolean",true]"#,
            r#"["boolean",false]"#,
            r#"["integer","0"]"#,
            r#"["integer","-1"]"#,
            r#"["integer","9223372036854775807"]"#,
            r#"["integer","-9223372036854775808"]"#,
            r#"["float","0x0.0p+0"]"#,
            r#"["float","-0x0.0p+0"]"#,
            r#"["float","0x1.921fb54442d18p+1"]"#,
            r#"["float","0x1.56e1fc2f8f359p-997"]"#,
            r#"["float","0x1.7e43c8800759cp+996"]"#,
            r#"["float","0x0.0000000000001p-1022"]"#,
            r#"["float","0x1.fffffffffffffp+1023"]"#,
            r#"["float","-0x1.c0302807c0200p+12"]"#,
            r#"["string",""]"#,
            "[\"string\",\"a\\tb\\nc\"]",
            r#"["string","\u0001"]"#,
            r#"["string","😀"]"#,
            r#"["string"," "]"#,
            r#"["date","2026-01-02"]"#,
            r#"["date","0001-01-01"]"#,
            r#"["date","9999-12-31"]"#,
            r#"["time","00:00:00.000000"]"#,
            r#"["time","23:59:59.999999"]"#,
            r#"["datetime","2026-01-02T03:04:05.000006"]"#,
            r#"["datetime","2026-01-02T00:02:03.000000+00:00"]"#,
            r#"["datetime","2025-12-31T23:00:00.000000+00:00"]"#,
            r#"["object",[]]"#,
            r#"["array",[]]"#,
            r#"["object",[["A",["integer","4"]],["a",["integer","2"]],["b",["integer","1"]],["é",["integer","3"]]]]"#,
            r#"["array",[["integer","1"],["object",[["x",["array",[["null"],["boolean",true]]]]]]]]"#,
        ];
        for text in canonical {
            let value = Value::from_semantic_bytes(text.as_bytes())
                .unwrap_or_else(|failure| panic!("must accept {text}: {failure}"));
            assert_eq!(encoded(&value), text);
        }
    }

    #[test]
    fn rejects_the_records_python_rejects() {
        let rejected = [
            r#"["integer","01"]"#,
            r#"["integer","+1"]"#,
            r#"["integer"," 1"]"#,
            r#"["integer","1_0"]"#,
            r#"["integer","9223372036854775808"]"#,
            r#"["float","0x1.8p+0"]"#,
            r#"["float","1.0"]"#,
            r#"["float","nan"]"#,
            r#"["float","inf"]"#,
            r#"["float","0X1.0000000000000P+0"]"#,
            r#"["date","2026-1-02"]"#,
            r#"["date","20260102"]"#,
            r#"["time","01:02:03"]"#,
            r#"["time","01:02:03.0"]"#,
            r#"["datetime","2026-01-02T00:02:03+00:00"]"#,
            r#"["datetime","2026-01-02T05:02:03.000000+05:00"]"#,
            r#"["datetime","2026-01-02T00:02:03.000000Z"]"#,
            r#"["datetime","2026-01-02 00:02:03.000000"]"#,
            r#"["boolean","true"]"#,
            r#"["boolean",1]"#,
            r#"["null",1]"#,
            r#"["nope","1"]"#,
            r#"["integer"]"#,
            "[]",
            r#"["string",1]"#,
            r#"["object",[["a",["integer","1"]],["a",["integer","2"]]]]"#,
            r#"["array",["integer","1"]]"#,
            r#"["object",[["a"]]]"#,
            r#"["object",[[]]]"#,
            r#"["null",null,null]"#,
            "[1]",
            r#""notalist""#,
            r#"{"a":1}"#,
        ];
        for text in rejected {
            assert!(
                Value::from_semantic_bytes(text.as_bytes()).is_err(),
                "must reject {text}"
            );
        }
        let unsorted = r#"["object",[["b",["integer","1"]],["a",["integer","2"]]]]"#;
        let value = Value::from_semantic_bytes(unsorted.as_bytes()).unwrap();
        assert_eq!(
            encoded(&value),
            r#"["object",[["a",["integer","2"]],["b",["integer","1"]]]]"#
        );
    }

    #[test]
    fn matches_the_shared_semantic_value_fixtures() {
        let Some(document) = fixture("configurations.json") else {
            eprintln!("skipping: spec fixtures are not available");
            return;
        };
        let cases = document
            .get("semantic_values")
            .and_then(Json::as_array)
            .unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let record = case.get("record").unwrap();
            let expected_hex = case.get("encoding_hex").and_then(Json::as_str).unwrap();
            let expected_digest = case.get("sha256").and_then(Json::as_str).unwrap();
            let value = super::value_from_record(record).unwrap();
            let bytes = value.semantic_bytes();
            assert_eq!(
                hex(&bytes),
                expected_hex,
                "encoding mismatch for {expected_hex}"
            );
            assert_eq!(hex(&Sha256::digest(&bytes)), expected_digest);
        }
    }
}
