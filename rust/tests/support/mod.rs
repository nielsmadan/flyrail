#![allow(dead_code)]

use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;
use std::path::Path;
use unicode_normalization::UnicodeNormalization as _;

pub fn destination_key(destination: &Path) -> String {
    let folded = caseless::default_case_fold_str(&destination.to_string_lossy())
        .nfc()
        .collect::<String>();
    Sha256::digest(folded.as_bytes())
        .iter()
        .fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

pub fn resource_key(destination: &Path, record: &str) -> String {
    format!("resource/{}/{record}", destination_key(destination))
}
