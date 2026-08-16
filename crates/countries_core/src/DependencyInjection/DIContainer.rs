//
//  DIContainer.swift
//  CountriesSwiftUI
//
//  Created by Alexey on 7/11/24.
//  Copyright © 2024 Alexey Naumov. All rights reserved.
//
//

#![allow(non_snake_case)]

use crate::Core::AppState::AppState;
use crate::Interactors::CountriesInteractor::{CountriesInteractor, StubCountriesInteractor};
use crate::Interactors::ImagesInteractor::{ImagesInteractor, StubImagesInteractor};
use crate::Interactors::UserPermissionsInteractor::{
    StubUserPermissionsInteractor, UserPermissionsInteractor,
};
use crate::Repositories::Database::CountriesDBRepository::CountriesDBRepository;
use crate::Repositories::WebAPI::CountriesWebRepository::CountriesWebRepository;
use crate::Repositories::WebAPI::ImagesWebRepository::ImagesWebRepository;
use crate::Repositories::WebAPI::PushTokenWebRepository::PushTokenWebRepository;
use motor::combine::Store;
use motor::state::{EnvironmentValues, FromEnvironment};
use std::rc::Rc;

/// `struct DIContainer` — resolved via `@Environment(\.injected)`.
#[derive(Clone)]
pub struct DIContainer {
    pub appState: Store<AppState>,
    pub interactors: Interactors,
}

impl DIContainer {
    /// `init(appState:interactors:)`
    pub fn new(appState: Store<AppState>, interactors: Interactors) -> Self {
        DIContainer { appState, interactors }
    }

    /// o default do `@Entry var injected` (`DIContainer(appState:,
    /// interactors: .stub)`).
    pub fn stub() -> DIContainer {
        DIContainer::new(Store::new(AppState::default()), Interactors::stub())
    }

    /// What gets stored in `EnvironmentValues.injected` (`Rc<dyn Any>`).
    pub fn shared(container: DIContainer) -> Rc<dyn std::any::Any> {
        Rc::new(container)
    }
}

/// `extension DIContainer { struct WebRepositories }`
#[derive(Clone)]
pub struct WebRepositories {
    pub images: Rc<dyn ImagesWebRepository>,
    pub countries: Rc<dyn CountriesWebRepository>,
    pub pushToken: Rc<dyn PushTokenWebRepository>,
}

/// `extension DIContainer { struct DBRepositories }`
#[derive(Clone)]
pub struct DBRepositories {
    pub countries: Rc<dyn CountriesDBRepository>,
}

/// `extension DIContainer { struct Interactors }`
#[derive(Clone)]
pub struct Interactors {
    pub images: Rc<dyn ImagesInteractor>,
    pub countries: Rc<dyn CountriesInteractor>,
    pub userPermissions: Rc<dyn UserPermissionsInteractor>,
}

impl Interactors {
    /// `static var stub: Self`
    pub fn stub() -> Interactors {
        Interactors {
            images: Rc::new(StubImagesInteractor),
            countries: Rc::new(StubCountriesInteractor),
            userPermissions: Rc::new(StubUserPermissionsInteractor),
        }
    }
}

impl FromEnvironment for DIContainer {
    fn from_environment(values: &EnvironmentValues) -> Self {
        let any = values.injected.clone().expect("\\.injected not present in the environment");
        let container = any.downcast::<DIContainer>().expect("\\.injected is not a DIContainer");
        (*container).clone()
    }
}
