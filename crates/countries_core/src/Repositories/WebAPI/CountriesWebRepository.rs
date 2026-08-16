//
//  CountriesWebRepository.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use super::WebRepository::{
    call, success, APICall, HTTPURLResponse, UrlSession, URLRequest, WebRepository,
};
use crate::Foundation::URL;
use crate::Repositories::Models::ApiModel;
use crate::Repositories::Models::DBModel;
use motor::loadable::LoadError;
use std::collections::HashMap;
use std::rc::Rc;

/// `protocol CountriesWebRepository`
pub trait CountriesWebRepository: WebRepository {
    fn countries(&self) -> super::WebRepository::BoxFuture<Result<Vec<ApiModel::Country>, LoadError>>;
    fn details(
        &self,
        country: &DBModel::Country,
    ) -> super::WebRepository::BoxFuture<Result<ApiModel::CountryDetails, LoadError>>;
}

#[derive(Clone)]
pub struct RealCountriesWebRepository {
    pub session: Rc<dyn UrlSession>,
    pub baseURL: String,
}

impl RealCountriesWebRepository {
    pub fn new(session: Rc<dyn UrlSession>) -> Self {
        RealCountriesWebRepository {
            session,
            baseURL: "https://restcountries.com/v2".into(),
        }
    }
}

impl WebRepository for RealCountriesWebRepository {
    fn session(&self) -> Rc<dyn UrlSession> {
        self.session.clone()
    }
    fn baseURL(&self) -> String {
        self.baseURL.clone()
    }
}

impl CountriesWebRepository for RealCountriesWebRepository {
    fn countries(&self) -> super::WebRepository::BoxFuture<Result<Vec<ApiModel::Country>, LoadError>> {
        // `try await call(self, …)` — o repository é clonado para o future
        // (BoxFuture é 'static).
        let repository = self.clone();
        Box::pin(async move {
            let value: Vec<ApiModel::Country> =
                call(&repository, &API::AllCountries, success()).await?;
            Ok(value)
        })
    }

    fn details(
        &self,
        country: &DBModel::Country,
    ) -> super::WebRepository::BoxFuture<Result<ApiModel::CountryDetails, LoadError>> {
        let repository = self.clone();
        let endpoint = API::CountryDetails { countryName: country.name.clone() };
        Box::pin(async move {
            let response: Vec<ApiModel::CountryDetails> =
                call(&repository, &endpoint, success()).await?;
            let Some(details) = response.into_iter().next() else {
                return Err(super::WebRepository::APIError::UnexpectedResponse.into());
            };
            Ok(details)
        })
    }
}

// MARK: - Endpoints

/// `extension RealCountriesWebRepository { enum API }`
pub enum API {
    AllCountries,
    CountryDetails { countryName: String },
}

impl APICall for API {
    fn path(&self) -> String {
        match self {
            API::AllCountries => {
                "/all?fields=name,translations,population,flag,alpha3Code".into()
            }
            API::CountryDetails { countryName } => {
                let encodedName = addingPercentEncoding(countryName);
                format!("/name/{}", encodedName.unwrap_or_else(|| countryName.clone()))
            }
        }
    }

    fn method(&self) -> String {
        match self {
            API::AllCountries | API::CountryDetails { .. } => "GET".into(),
        }
    }

    fn headers(&self) -> Option<HashMap<String, String>> {
        Some(HashMap::from([("Accept".into(), "application/json".into())]))
    }

    fn body(&self) -> Result<Option<Vec<u8>>, LoadError> {
        Ok(None)
    }
}

/// `addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed)` — o
/// mínimo que o endpoint precisa (letras/dígitos passam, espaços viram %20).
fn addingPercentEncoding(string: &str) -> Option<String> {
    let mut out = String::new();
    for byte in string.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    Some(out)
}

// MARK: - MockUrlSession

/// `MockUrlSession` — serve o JSON serializado do `MockedData`; o pipeline de
/// decode roda de verdade.
pub struct MockUrlSession;

impl UrlSession for MockUrlSession {
    fn data(
        &self,
        request: URLRequest,
    ) -> super::WebRepository::BoxFuture<Result<(Vec<u8>, HTTPURLResponse), LoadError>> {
        Box::pin(async move {
            let path = request.url.absoluteString;
            let json = if path.contains("/all?") {
                serde_json::to_vec(&ApiModel::mockedCountries()).unwrap()
            } else if let Some(name) = path.strip_prefix("https://restcountries.com/v2/name/") {
                let decoded = name.split('%').next().unwrap_or(name);
                let details = ApiModel::mockedCountryDetails();
                let chosen = details
                    .into_iter()
                    .find(|d| d.capital.to_lowercase().contains(&decoded.to_lowercase()))
                    .unwrap_or(ApiModel::CountryDetails {
                        capital: decoded.to_string(),
                        currencies: vec![],
                        borders: Some(vec![]),
                    });
                serde_json::to_vec(&vec![chosen]).unwrap()
            } else {
                vec![]
            };
            Ok((json, HTTPURLResponse { statusCode: 200 }))
        })
    }

    fn download(&self, _url: URL) -> super::WebRepository::BoxFuture<Result<Vec<u8>, LoadError>> {
        // `UIImage(data:)` fake: qualquer byte vira a imagem unitária
        Box::pin(async { Ok(vec![0u8; 4]) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allCountriesEndpoint() {
        let endpoint = API::AllCountries;
        assert_eq!(endpoint.method(), "GET");
        assert!(endpoint.path().starts_with("/all?fields="));
        let headers = endpoint.headers().unwrap();
        assert_eq!(headers["Accept"], "application/json");
    }

    #[test]
    fn countryDetailsPathIsEncoded() {
        let endpoint = API::CountryDetails { countryName: "United States".into() };
        assert_eq!(endpoint.path(), "/name/United%20States");
    }

    #[test]
    fn mockSessionServesDecodableCountries() {
        let repository = RealCountriesWebRepository::new(Rc::new(MockUrlSession));
        let countries = motor::block_on(repository.countries()).unwrap();
        assert_eq!(countries.len(), 3);
        assert_eq!(countries[0].alpha3Code, "USA");
        assert!(countries[0].flag.is_some());
    }
}
