//
//  PushTokenWebRepository.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use super::WebRepository::{UrlSession, WebRepository};
use motor::loadable::LoadError;
use std::rc::Rc;

/// `protocol PushTokenWebRepository`
pub trait PushTokenWebRepository: WebRepository {
    fn register(&self, devicePushToken: Vec<u8>) -> super::WebRepository::BoxFuture<Result<(), LoadError>>;
}

pub struct RealPushTokenWebRepository {
    pub session: Rc<dyn UrlSession>,
    pub baseURL: String,
}

impl RealPushTokenWebRepository {
    pub fn new(session: Rc<dyn UrlSession>) -> Self {
        RealPushTokenWebRepository {
            session,
            baseURL: "https://your-server.com/api/push-token".into(),
        }
    }
}

impl WebRepository for RealPushTokenWebRepository {
    fn session(&self) -> Rc<dyn UrlSession> {
        self.session.clone()
    }
    fn baseURL(&self) -> String {
        self.baseURL.clone()
    }
}

impl PushTokenWebRepository for RealPushTokenWebRepository {
    fn register(
        &self,
        _devicePushToken: Vec<u8>,
    ) -> super::WebRepository::BoxFuture<Result<(), LoadError>> {
        // upload the push token to your server
        // you can as well call a third party library here instead
        Box::pin(async { Ok(()) })
    }
}
