//
//  ImagesWebRepository.swift (arquivo original: ImageWebRepository.swift)
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use super::WebRepository::{UrlSession, WebRepository};
use crate::Foundation::{URL, UIImage};
use motor::loadable::LoadError;
use std::rc::Rc;

/// `protocol ImagesWebRepository`
pub trait ImagesWebRepository: WebRepository {
    fn loadImage(&self, url: URL) -> super::WebRepository::BoxFuture<Result<UIImage, LoadError>>;
}

pub struct RealImagesWebRepository {
    pub session: Rc<dyn UrlSession>,
    pub baseURL: String,
}

impl RealImagesWebRepository {
    pub fn new(session: Rc<dyn UrlSession>) -> Self {
        RealImagesWebRepository { session, baseURL: String::new() }
    }
}

impl WebRepository for RealImagesWebRepository {
    fn session(&self) -> Rc<dyn UrlSession> {
        self.session.clone()
    }
    fn baseURL(&self) -> String {
        self.baseURL.clone()
    }
}

impl ImagesWebRepository for RealImagesWebRepository {
    fn loadImage(&self, url: URL) -> super::WebRepository::BoxFuture<Result<UIImage, LoadError>> {
        // o Rc do session é clonado para o future (BoxFuture é 'static)
        let session = self.session();
        Box::pin(async move {
            // `session.download` → `Data(contentsOf:)` → `UIImage(data:)` —
            // no fake qualquer payload desserializa na UIImage unitária.
            let data = session.download(url).await?;
            if data.is_empty() {
                return Err(super::WebRepository::APIError::ImageDeserialization.into());
            }
            Ok(UIImage)
        })
    }
}
