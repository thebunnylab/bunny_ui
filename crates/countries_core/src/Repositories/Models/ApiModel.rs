//
//  ApiModel.rs — the web API models (Country.swift / CountryDetails.swift /
//  CountryCurrency.swift / MockedData.swift, `extension ApiModel` parts).
//

#![allow(non_snake_case)]

use crate::Foundation::URL;
use serde_json::{Map, Value};
use motor::loadable::LoadError;

/// `enum ApiModel { }` — a namespace in Swift, a module here.
///
/// `CodingKeys.flag = "alpha2Code"` and the custom `init(from:)` are ported
/// as manual `Serialize`/`Deserialize` impls: a 2-char `alpha2Code` becomes a
/// flagcdn URL, anything else is used as the URL directly.
#[derive(Clone, Debug, PartialEq)]
pub struct Country {
    pub name: String,
    pub translations: std::collections::HashMap<String, Option<String>>,
    pub population: i64,
    pub flag: Option<URL>,
    pub alpha3Code: String,
}

impl Country {
    pub fn new(
        name: &str,
        translations: std::collections::HashMap<String, Option<String>>,
        population: i64,
        flag: Option<URL>,
        alpha3Code: &str,
    ) -> Self {
        Country {
            name: name.into(),
            translations,
            population,
            flag,
            alpha3Code: alpha3Code.into(),
        }
    }

    /// `init(from decoder:)` — `alpha2Code` (2 chars) → flagcdn URL.
    fn fromJSON(json: &Value) -> Result<Self, LoadError> {
        let object = json.as_object().ok_or_else(|| LoadError::new("unexpected response"))?;
        let get = |key: &str| {
            object.get(key).and_then(Value::as_str).map(str::to_string)
        };
        let translations = match object.get("translations") {
            Some(Value::Object(map)) => decodeTranslations(map),
            _ => std::collections::HashMap::new(),
        };
        let flag = get("alpha2Code").map(|alpha2orFlagURL| {
            let urlString = if alpha2orFlagURL.chars().count() == 2 {
                format!("https://flagcdn.com/w640/{}.jpg", alpha2orFlagURL.to_lowercase())
            } else {
                alpha2orFlagURL
            };
            URL::new(urlString)
        });
        Ok(Country {
            name: get("name").unwrap_or_default(),
            translations,
            population: object.get("population").and_then(Value::as_i64).unwrap_or(0),
            flag: flag.flatten(),
            alpha3Code: get("alpha3Code").unwrap_or_default(),
        })
    }

    /// `encode(to:)` — mirrors `init(from:)` so the mocked data round-trips
    /// through the JSON pipeline (the mock URL session serves this).
    fn toJSON(&self) -> Value {
        let mut object = Map::new();
        object.insert("name".into(), Value::String(self.name.clone()));
        let mut translations = Map::new();
        let mut keys: Vec<_> = self.translations.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let value = self.translations[&key].clone();
            object_insert_translation(&mut translations, &key, value);
        }
        object.insert("translations".into(), Value::Object(translations));
        object.insert("population".into(), Value::from(self.population));
        // flag URL → alpha2Code (the last path component without extension)
        let alpha2 = self.flag.as_ref().map(|url| {
            url.absoluteString
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .split('.')
                .next()
                .unwrap_or_default()
                .to_string()
        });
        match alpha2 {
            Some(code) if !code.is_empty() => {
                object.insert("alpha2Code".into(), Value::String(code));
            }
            _ => {
                object.insert("alpha2Code".into(), Value::Null);
            }
        }
        object.insert("alpha3Code".into(), Value::String(self.alpha3Code.clone()));
        Value::Object(object)
    }
}

/// `[String: String?]` — null and missing both decode to `None`.
fn decodeTranslations(map: &Map<String, Value>) -> std::collections::HashMap<String, Option<String>> {
    map.iter()
        .map(|(key, value)| {
            let decoded = match value {
                Value::String(s) => Some(s.clone()),
                _ => None,
            };
            (key.clone(), decoded)
        })
        .collect()
}

fn object_insert_translation(map: &mut Map<String, Value>, key: &str, value: Option<String>) {
    match value {
        Some(s) => map.insert(key.into(), Value::String(s)),
        None => map.insert(key.into(), Value::Null),
    };
}

/// `struct CountryDetails: Codable` — `capital`, `currencies`, `borders`.
#[derive(Clone, Debug, PartialEq)]
pub struct CountryDetails {
    pub capital: String,
    pub currencies: Vec<Currency>,
    pub borders: Option<Vec<String>>,
}

/// `struct Currency: Codable` — `code`, `symbol`, `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Currency {
    pub code: String,
    pub symbol: Option<String>,
    pub name: String,
}

// MARK: - Codable (serde against the hand-rolled JSON shapes)

impl serde::Serialize for Country {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde_json::Value::serialize(&self.toJSON(), serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Country {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = Value::deserialize(deserializer)?;
        Country::fromJSON(&json).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for CountryDetails {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut object = Map::new();
        object.insert("capital".into(), Value::String(self.capital.clone()));
        object.insert(
            "currencies".into(),
            Value::Array(self.currencies.iter().map(currencyToJSON).collect()),
        );
        match &self.borders {
            Some(borders) => {
                object.insert(
                    "borders".into(),
                    Value::Array(borders.iter().map(|b| Value::String(b.clone())).collect()),
                );
            }
            None => {
                object.insert("borders".into(), Value::Null);
            }
        }
        Value::Object(object).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for CountryDetails {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = Value::deserialize(deserializer)?;
        let object = json.as_object().ok_or_else(|| serde::de::Error::custom("not an object"))?;
        let capital = object
            .get("capital")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let currencies = object
            .get("currencies")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(currencyFromJSON)
                    .collect::<Vec<Currency>>()
            })
            .unwrap_or_default();
        let borders = object.get("borders").and_then(Value::as_array).map(|list| {
            list.iter().filter_map(Value::as_str).map(str::to_string).collect()
        });
        Ok(CountryDetails { capital, currencies, borders })
    }
}

impl serde::Serialize for Currency {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Value::serialize(&currencyToJSON(self), serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Currency {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = Value::deserialize(deserializer)?;
        currencyFromJSON(&json).ok_or_else(|| serde::de::Error::custom("not a currency"))
    }
}

fn currencyToJSON(currency: &Currency) -> Value {
    let mut object = Map::new();
    object.insert("code".into(), Value::String(currency.code.clone()));
    match &currency.symbol {
        Some(symbol) => object.insert("symbol".into(), Value::String(symbol.clone())),
        None => object.insert("symbol".into(), Value::Null),
    };
    object.insert("name".into(), Value::String(currency.name.clone()));
    Value::Object(object)
}

fn currencyFromJSON(json: &Value) -> Option<Currency> {
    let object = json.as_object()?;
    Some(Currency {
        code: object.get("code")?.as_str()?.to_string(),
        symbol: object.get("symbol").and_then(Value::as_str).map(str::to_string),
        name: object.get("name")?.as_str()?.to_string(),
    })
}

// MARK: - MockedData.swift

/// `extension ApiModel.Country { static let mockedData }`
pub fn mockedCountries() -> Vec<Country> {
    vec![
        Country::new("United States", std::collections::HashMap::new(), 125_000_000,
            URL::new("https://flagcdn.com/w640/us.jpg".into()), "USA"),
        Country::new("Georgia", std::collections::HashMap::new(), 2_340_000, None, "GEO"),
        Country::new("Canada", std::collections::HashMap::new(), 57_600_000, None, "CAN"),
    ]
}

/// `extension ApiModel.CountryDetails { static var mockedData }`
pub fn mockedCountryDetails() -> Vec<CountryDetails> {
    vec![
        CountryDetails {
            capital: "Sin City".into(),
            currencies: mockedCurrencies(),
            borders: Some(vec!["abc".into()]),
        },
        CountryDetails {
            capital: "Los Angeles".into(),
            currencies: mockedCurrencies(),
            borders: Some(vec![]),
        },
        CountryDetails {
            capital: "New York".into(),
            currencies: vec![],
            borders: Some(vec![]),
        },
        CountryDetails {
            capital: "Moscow".into(),
            currencies: vec![],
            borders: Some(vec!["xyz".into()]),
        },
    ]
}

/// `extension ApiModel.Currency { static let mockedData }`
pub fn mockedCurrencies() -> Vec<Currency> {
    vec![
        Currency { code: "USD".into(), symbol: Some("$".into()), name: "US Dollar".into() },
        Currency { code: "EUR".into(), symbol: Some("€".into()), name: "Euro".into() },
        Currency { code: "RUB".into(), symbol: Some("‡".into()), name: "Rouble".into() },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countryRoundTripsThroughJSON() {
        let countries = mockedCountries();
        let json = serde_json::to_string(&countries).unwrap();
        let decoded: Vec<Country> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, countries);
    }

    #[test]
    fn alpha2BecomesFlagcdnURL() {
        let json = r#"[{"name":"Test","translations":{},"population":1,
            "alpha2Code":"us","alpha3Code":"USA"}]"#;
        let decoded: Vec<Country> = serde_json::from_str(json).unwrap();
        assert_eq!(
            decoded[0].flag.as_ref().map(|u| u.absoluteString.clone()),
            Some("https://flagcdn.com/w640/us.jpg".to_string())
        );
    }

    #[test]
    fn detailsDecode() {
        let json = r#"{"capital":"Sin City","currencies":[{"code":"USD","symbol":"$","name":"US Dollar"}],"borders":["abc"]}"#;
        let decoded: CountryDetails = serde_json::from_str(json).unwrap();
        assert_eq!(decoded.capital, "Sin City");
        assert_eq!(decoded.currencies.len(), 1);
        assert_eq!(decoded.borders, Some(vec!["abc".to_string()]));
    }
}
