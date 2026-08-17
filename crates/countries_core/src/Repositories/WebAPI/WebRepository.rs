//
//  WebRepository.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use crate::Foundation::URL;
use motor::loadable::LoadError;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

/// `async throws` in a trait: boxed future (single-thread, like the whole app).
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;

pub type HTTPCode = i64;
pub type HTTPCodes = std::ops::Range<HTTPCode>;

/// `extension HTTPCodes { static let success = 200..<300 }`
pub fn success() -> HTTPCodes {
    200..300
}

// MARK: - APICall

/// `protocol APICall` — path/method/headers/body.
pub trait APICall {
    fn path(&self) -> String;
    fn method(&self) -> String;
    fn headers(&self) -> Option<HashMap<String, String>>;
    fn body(&self) -> Result<Option<Vec<u8>>, LoadError>;
}

impl APICall for () {
    fn path(&self) -> String {
        String::new()
    }
    fn method(&self) -> String {
        "GET".into()
    }
    fn headers(&self) -> Option<HashMap<String, String>> {
        None
    }
    fn body(&self) -> Result<Option<Vec<u8>>, LoadError> {
        Ok(None)
    }
}

/// Fake `URLRequest` — everything `urlRequest(baseURL:)` assembles.
#[derive(Clone, Debug)]
pub struct URLRequest {
    pub url: URL,
    pub httpMethod: String,
    pub allHTTPHeaderFields: Option<HashMap<String, String>>,
    pub httpBody: Option<Vec<u8>>,
}

/// `extension APICall { func urlRequest(baseURL:) throws -> URLRequest }`
pub fn urlRequest(call: &dyn APICall, baseURL: &str) -> Result<URLRequest, LoadError> {
    let Some(url) = URL::new(format!("{baseURL}{}", call.path())) else {
        return Err(APIError::InvalidURL.into());
    };
    Ok(URLRequest {
        url,
        httpMethod: call.method(),
        allHTTPHeaderFields: call.headers(),
        httpBody: call.body()?,
    })
}

// MARK: - APIError

/// `enum APIError: Swift.Error, Equatable`
#[derive(Clone, Debug, PartialEq)]
pub enum APIError {
    InvalidURL,
    HttpCode(HTTPCode),
    UnexpectedResponse,
    ImageDeserialization,
}

impl APIError {
    /// `var errorDescription: String?`
    pub fn errorDescription(&self) -> String {
        match self {
            APIError::InvalidURL => "Invalid URL".into(),
            APIError::HttpCode(code) => format!("Unexpected HTTP code: {code}"),
            APIError::UnexpectedResponse => "Unexpected response from the server".into(),
            APIError::ImageDeserialization => "Cannot deserialize image from Data".into(),
        }
    }
}

impl std::fmt::Display for APIError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.errorDescription())
    }
}

impl From<APIError> for LoadError {
    fn from(error: APIError) -> LoadError {
        LoadError::new(error.errorDescription())
    }
}

// MARK: - URLSession fake

/// `HTTPURLResponse.statusCode`
#[derive(Clone, Debug)]
pub struct HTTPURLResponse {
    pub statusCode: HTTPCode,
}

/// Fake `URLSession` — a trait for the MockUrlSession (and a real one that does no networking).
pub trait UrlSession {
    /// `session.data(for:)`
    fn data(&self, request: URLRequest) -> BoxFuture<Result<(Vec<u8>, HTTPURLResponse), LoadError>>;
    /// `session.download(from:)` — returns the bytes (the fake UIImage is unit).
    fn download(&self, url: URL) -> BoxFuture<Result<Vec<u8>, LoadError>>;
}

/// `URLSession(configuration:)` — in the headless port no networking happens.
pub struct RealUrlSession;

impl UrlSession for RealUrlSession {
    fn data(&self, _request: URLRequest) -> BoxFuture<Result<(Vec<u8>, HTTPURLResponse), LoadError>> {
        Box::pin(async { Err(APIError::UnexpectedResponse.into()) })
    }
    fn download(&self, _url: URL) -> BoxFuture<Result<Vec<u8>, LoadError>> {
        Box::pin(async { Err(APIError::UnexpectedResponse.into()) })
    }
}

// MARK: - WebRepository

/// `protocol WebRepository { var session; var baseURL }`
pub trait WebRepository {
    fn session(&self) -> Rc<dyn UrlSession>;
    fn baseURL(&self) -> String;
}

/// `extension WebRepository { func call<Value>(endpoint:decoder:httpCodes:) }` —
/// Swift's generic method with a default implementation becomes this free fn
/// (Rust has no generic default method on a trait object).
pub async fn call<Value: serde::de::DeserializeOwned>(
    repository: &dyn WebRepository,
    endpoint: &dyn APICall,
    httpCodes: HTTPCodes,
) -> Result<Value, LoadError> {
    let request = urlRequest(endpoint, &repository.baseURL())?;
    let (data, response) = repository.session().data(request).await?;
    let code = response.statusCode;
    if !httpCodes.contains(&code) {
        return Err(APIError::HttpCode(code).into());
    }
    match serde_json::from_slice::<Value>(&data) {
        Ok(value) => Ok(value),
        Err(_) => Err(APIError::UnexpectedResponse.into()),
    }
}
