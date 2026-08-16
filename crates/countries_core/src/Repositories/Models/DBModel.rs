//
//  Country.swift (DBModel part)
//  CountriesSwiftUI
//

use crate::Foundation::URL;
use std::collections::HashMap;
use motor::state::Locale;

// MARK: - Database Model

/// `DBModel.Country` — SwiftData `@Model` falso: um struct plain.
#[derive(Clone, Debug, PartialEq)]
pub struct Country {
    pub name: String,
    pub translations: HashMap<String, Option<String>>,
    pub population: i64,
    pub flag: Option<URL>,
    pub alpha3Code: String,
}

impl Country {
    pub fn new(
        name: String,
        translations: HashMap<String, Option<String>>,
        population: i64,
        flag: Option<URL>,
        alpha3Code: String,
    ) -> Self {
        Country { name, translations, population, flag, alpha3Code }
    }

    /// `func name(locale: Locale) -> String`
    pub fn name_locale(&self, locale: Locale) -> String {
        let localeId = locale.shortIdentifier();
        if let Some(value) = self.translations.get(&localeId) {
            if let Some(localizedName) = value {
                return localizedName.clone();
            }
        }
        self.name.clone()
    }
}

/// `DBModel.CountryDetails` (CountryDetails.swift) — `neighbors` keeps the
/// Swift `Optional<Array>` shape.
#[derive(Clone, Debug, PartialEq)]
pub struct CountryDetails {
    pub alpha3Code: String,
    pub capital: String,
    pub currencies: Vec<Currency>,
    pub neighbors: Option<Vec<Country>>,
}

impl CountryDetails {
    pub fn new(
        alpha3Code: String,
        capital: String,
        currencies: Vec<Currency>,
        neighbors: Vec<Country>,
    ) -> Self {
        CountryDetails {
            alpha3Code,
            capital,
            currencies,
            neighbors: Some(neighbors),
        }
    }
}

/// `DBModel.Currency` (CountryCurrency.swift)
#[derive(Clone, Debug, PartialEq)]
pub struct Currency {
    pub code: String,
    pub symbol: Option<String>,
    pub name: String,
}

impl Currency {
    /// `var title: String { name + (symbol.map { " " + $0 } ?? "") }`
    /// (CountryDetailsView private extension)
    pub fn title(&self) -> String {
        let symbol = self.symbol.as_ref().map(|s| format!(" {s}"));
        format!("{}{}", self.name, symbol.unwrap_or_default())
    }
}
