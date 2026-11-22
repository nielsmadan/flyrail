use flyrail::{
    AuthorityError, BundleError, HookFailure, StoreError, UnsupportedTranslation, ValueError,
};
use napi::Env;
use napi::bindgen_prelude::{Object, ToNapiValue};
use napi_derive::napi;
use std::path::Path;

pub(crate) enum BindingError {
    Value(ValueError),
    Bundle(BundleError),
    Authority(AuthorityError),
    Store(StoreError),
    Hook(HookFailure),
    UnsupportedTranslation(UnsupportedTranslation),
    Argument(String),
    Aborted,
    Panic(String),
    Native(String),
}

pub(crate) type BindingResult<T> = Result<T, BindingError>;

impl From<ValueError> for BindingError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}

impl From<BundleError> for BindingError {
    fn from(error: BundleError) -> Self {
        Self::Bundle(error)
    }
}

impl From<AuthorityError> for BindingError {
    fn from(error: AuthorityError) -> Self {
        Self::Authority(error)
    }
}

impl From<StoreError> for BindingError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<HookFailure> for BindingError {
    fn from(error: HookFailure) -> Self {
        Self::Hook(error)
    }
}

impl From<UnsupportedTranslation> for BindingError {
    fn from(error: UnsupportedTranslation) -> Self {
        Self::UnsupportedTranslation(error)
    }
}

#[napi(object)]
pub struct NativeErrorPayload {
    #[napi(js_name = "type")]
    pub tag: String,
    pub kind: Option<String>,
    pub message: Option<String>,
    pub code: Option<String>,
    pub path: Option<String>,
    pub lock_name: Option<String>,
    pub errno: Option<i32>,
}

impl NativeErrorPayload {
    fn new(tag: &str) -> Self {
        Self {
            tag: tag.to_owned(),
            kind: None,
            message: None,
            code: None,
            path: None,
            lock_name: None,
            errno: None,
        }
    }

    fn kind(self, kind: &str) -> Self {
        Self {
            kind: Some(kind.to_owned()),
            ..self
        }
    }

    fn message(self, message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..self
        }
    }

    fn code(self, code: impl Into<String>) -> Self {
        Self {
            code: Some(code.into()),
            ..self
        }
    }

    fn path(self, path: Option<&Path>) -> Self {
        Self {
            path: path.map(|path| path.to_string_lossy().into_owned()),
            ..self
        }
    }

    fn errno(self, errno: Option<i32>) -> Self {
        Self { errno, ..self }
    }
}

fn bundle_payload(error: &BundleError) -> NativeErrorPayload {
    let payload = NativeErrorPayload::new("bundle");
    match error {
        BundleError::Invalid(error) => payload.kind("invalid").message(error.message()),
        BundleError::Io(error) => payload
            .kind("io")
            .message(error.to_string())
            .errno(error.raw_os_error()),
    }
}

fn authority_payload(error: &AuthorityError) -> NativeErrorPayload {
    let payload = NativeErrorPayload::new("authority")
        .message(error.message())
        .path(error.path());
    match error {
        AuthorityError::Invalid(_) => payload.kind("invalid"),
        AuthorityError::Unsafe { .. } => payload.kind("unsafe"),
        AuthorityError::Io { errno, .. } => payload.kind("io").errno(*errno),
    }
}

fn store_payload(error: &StoreError) -> NativeErrorPayload {
    let payload = NativeErrorPayload::new("store").message(error.to_string());
    match error {
        StoreError::Busy { name } => NativeErrorPayload {
            lock_name: Some(name.clone()),
            ..payload.kind("busy")
        },
        StoreError::Io { path, errno, .. } => {
            payload.kind("io").path(path.as_deref()).errno(*errno)
        }
        StoreError::InvalidKey { .. } => payload.kind("invalid-key"),
    }
}

impl From<&BindingError> for NativeErrorPayload {
    fn from(error: &BindingError) -> Self {
        match error {
            BindingError::Value(error) => Self::new("value").message(error.message()),
            BindingError::Bundle(error) => bundle_payload(error),
            BindingError::Authority(error) => authority_payload(error),
            BindingError::Store(error) => store_payload(error),
            BindingError::Hook(error) => Self::new("hook").code(error.code()),
            BindingError::UnsupportedTranslation(error) => Self::new("unsupported-translation")
                .code(error.code())
                .message(error.message()),
            BindingError::Argument(message) => Self::new("argument").message(message.as_str()),
            BindingError::Aborted => Self::new("aborted"),
            BindingError::Panic(message) => Self::new("panic").message(message.as_str()),
            BindingError::Native(message) => Self::new("native").message(message.as_str()),
        }
    }
}

pub(crate) fn tagged<J: ToNapiValue>(
    env: &Env,
    result: BindingResult<J>,
) -> napi::Result<Object<'_>> {
    let mut object = Object::new(env)?;
    match result {
        Ok(value) => {
            object.set("ok", true)?;
            object.set("value", value)?;
        }
        Err(error) => {
            object.set("ok", false)?;
            object.set("error", NativeErrorPayload::from(&error))?;
        }
    }
    Ok(object)
}
