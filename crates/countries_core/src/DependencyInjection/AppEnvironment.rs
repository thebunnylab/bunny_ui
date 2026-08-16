//
//  AppEnvironment.swift
//  CountriesSwiftUI
//
//  Created by Alexey on 7/11/24.
//  Copyright © 2024 Alexey Naumov. All rights reserved.
//
//

#![allow(non_snake_case)]

use crate::Core::AppState::AppState;
use crate::Core::DeepLinksHandler::{DeepLinksHandler, RealDeepLinksHandler};
use crate::Core::PushNotificationsHandler::{PushNotificationsHandler, RealPushNotificationsHandler};
use crate::Core::SystemEventsHandler::{RealSystemEventsHandler, SystemEventsHandler};
use crate::DependencyInjection::DIContainer::{
    DBRepositories, DIContainer, Interactors, WebRepositories,
};
use crate::Interactors::CountriesInteractor::RealCountriesInteractor;
use crate::Interactors::ImagesInteractor::RealImagesInteractor;
use crate::Interactors::UserPermissionsInteractor::RealUserPermissionsInteractor;
use crate::Repositories::Database::ModelContainer::{MainDBRepository, ModelContainer};
use crate::Repositories::WebAPI::CountriesWebRepository::{MockUrlSession, RealCountriesWebRepository};
use crate::Repositories::WebAPI::ImagesWebRepository::RealImagesWebRepository;
use crate::Repositories::WebAPI::PushTokenWebRepository::RealPushTokenWebRepository;
use crate::Repositories::WebAPI::WebRepository::UrlSession;
use crate::Utilities::Helpers::ProcessInfo;
use motor::combine::Store;
use std::rc::Rc;

/// `@MainActor struct AppEnvironment`
pub struct AppEnvironment {
    pub isRunningTests: bool,
    pub diContainer: DIContainer,
    pub modelContainer: ModelContainer,
    pub systemEventsHandler: Rc<dyn SystemEventsHandler>,
}

impl AppEnvironment {
    /// `static func bootstrap() -> AppEnvironment`
    pub fn bootstrap() -> AppEnvironment {
        let appState = Store::<AppState>::new(AppState::default());
        /*
         To see the deep linking in action (no fake do Swift era o simulador +
         "push_with_deeplink.apns"; aqui disparamos o deep link direto):

             deepLinksHandler.open(deepLink: .showCountryFlag(alpha3Code: "AFG"))
        */
        let session = Self::configuredURLSession();
        let webRepositories = Self::configuredWebRepositories(session);
        let modelContainer = Self::configuredModelContainer();
        let dbRepositories = Self::configuredDBRepositories(&modelContainer);
        let interactors = Self::configuredInteractors(&appState, &webRepositories, &dbRepositories);
        let diContainer = DIContainer::new(appState, interactors);
        let deepLinksHandler: Rc<dyn DeepLinksHandler> =
            Rc::new(RealDeepLinksHandler::new(diContainer.clone()));
        let pushNotificationsHandler: Rc<dyn PushNotificationsHandler> =
            Rc::new(RealPushNotificationsHandler::new(deepLinksHandler.clone()));
        let systemEventsHandler: Rc<dyn SystemEventsHandler> = Rc::new(RealSystemEventsHandler::new(
            diContainer.clone(),
            deepLinksHandler,
            pushNotificationsHandler,
            webRepositories.pushToken.clone(),
        ));
        AppEnvironment {
            isRunningTests: ProcessInfo::isRunningTests(),
            diContainer,
            modelContainer,
            systemEventsHandler,
        }
    }
}

/// `extension AppEnvironment { private static func … }`
impl AppEnvironment {
    /// `configuredURLSession()` — `URLSessionConfiguration.default` com
    /// timeouts etc; no fake a "rede" é o `MockUrlSession` servindo o
    /// `MockedData` (o pipeline de decode roda de verdade).
    fn configuredURLSession() -> Rc<dyn UrlSession> {
        Rc::new(MockUrlSession)
    }

    fn configuredWebRepositories(session: Rc<dyn UrlSession>) -> WebRepositories {
        let images = Rc::new(RealImagesWebRepository::new(session.clone()));
        let countries = Rc::new(RealCountriesWebRepository::new(session.clone()));
        let pushToken = Rc::new(RealPushTokenWebRepository::new(session));
        WebRepositories { images, countries, pushToken }
    }

    fn configuredDBRepositories(modelContainer: &ModelContainer) -> DBRepositories {
        let mainDBRepository = Rc::new(MainDBRepository::new(modelContainer.clone()));
        DBRepositories { countries: mainDBRepository }
    }

    /// `do { try ModelContainer.appModelContainer() } catch { .stub }`
    fn configuredModelContainer() -> ModelContainer {
        match ModelContainer::appModelContainer(false, false) {
            Ok(container) => container,
            Err(_) => ModelContainer::stub(),
        }
    }

    fn configuredInteractors(
        appState: &Store<AppState>,
        webRepositories: &WebRepositories,
        dbRepositories: &DBRepositories,
    ) -> Interactors {
        let images = Rc::new(RealImagesInteractor::new(webRepositories.images.clone()));
        let countries = Rc::new(RealCountriesInteractor::new(
            webRepositories.countries.clone(),
            dbRepositories.countries.clone(),
        ));
        let userPermissions = Rc::new(RealUserPermissionsInteractor::new(
            appState.clone(),
            // UIApplication.shared.open(openSettingsURLString) — no-op headless
            Rc::new(|| {}),
        ));
        Interactors { images, countries, userPermissions }
    }
}
