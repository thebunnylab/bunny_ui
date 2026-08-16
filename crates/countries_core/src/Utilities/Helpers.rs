//
//  Helpers.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use motor::state::Locale;

/// `extension ProcessInfo { var isRunningTests: Bool }`
pub struct ProcessInfo;

impl ProcessInfo {
    pub fn isRunningTests() -> bool {
        cfg!(test)
    }
}

/// `extension String { func localized(_ locale: Locale) -> String }` — no
/// bundle machinery here: returns the key itself (like a development locale).
pub trait StringLocalized {
    fn localized(&self, locale: &Locale) -> String;
}

impl StringLocalized for String {
    fn localized(&self, _locale: &Locale) -> String {
        self.clone()
    }
}

impl StringLocalized for &str {
    fn localized(&self, _locale: &Locale) -> String {
        (*self).to_string()
    }
}

/// `extension Locale { static var backendDefault: Locale }`
pub trait LocaleBackendDefault {
    fn backendDefault() -> Locale;
}

impl LocaleBackendDefault for Locale {
    fn backendDefault() -> Locale {
        Locale::new("en")
    }
}

/// `extension String { func localizedStandardContains(_:) -> Bool }` —
/// case/diacritics-insensitive contains (o filtro de busca da lista).
pub trait StringLocalizedStandardContains {
    fn localizedStandardContains(&self, other: impl AsRef<str>) -> bool;
}

impl StringLocalizedStandardContains for String {
    fn localizedStandardContains(&self, other: impl AsRef<str>) -> bool {
        let haystack = self.to_lowercase();
        let needle = other.as_ref().to_lowercase();
        haystack.contains(&needle)
    }
}

/// `extension Result { var isSuccess: Bool }`
pub trait ResultIsSuccess<T, E> {
    fn isSuccess(&self) -> bool;
}

impl<T, E> ResultIsSuccess<T, E> for Result<T, E> {
    fn isSuccess(&self) -> bool {
        matches!(self, Ok(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backendDefaultIsEn() {
        assert_eq!(Locale::backendDefault().identifier, "en");
    }

    #[test]
    fn resultIsSuccess() {
        let ok: Result<i32, String> = Ok(1);
        let err: Result<i32, String> = Err("e".into());
        assert!(ok.isSuccess());
        assert!(!err.isSuccess());
    }
}
