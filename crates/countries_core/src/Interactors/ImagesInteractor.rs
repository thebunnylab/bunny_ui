//
//  ImagesInteractor.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use crate::Foundation::{URL, UIImage};
use crate::Repositories::WebAPI::ImagesWebRepository::ImagesWebRepository;
use motor::loadable::{Loadable, LoadableSubject};
use std::rc::Rc;

/// `protocol ImagesInteractor { func load(image: LoadableSubject<UIImage>, url: URL?) }`
pub trait ImagesInteractor {
    fn load(&self, image: LoadableSubject<UIImage>, url: Option<URL>);
}

pub struct RealImagesInteractor {
    pub webRepository: Rc<dyn ImagesWebRepository>,
}

impl RealImagesInteractor {
    pub fn new(webRepository: Rc<dyn ImagesWebRepository>) -> Self {
        RealImagesInteractor { webRepository }
    }
}

impl ImagesInteractor for RealImagesInteractor {
    fn load(&self, image: LoadableSubject<UIImage>, url: Option<URL>) {
        let Some(url) = url else {
            image.set(Loadable::NotRequested);
            return;
        };
        let webRepository = self.webRepository.clone();
        image.load(webRepository.loadImage(url));
    }
}

pub struct StubImagesInteractor;

impl ImagesInteractor for StubImagesInteractor {
    fn load(&self, _image: LoadableSubject<UIImage>, _url: Option<URL>) {}
}
