//! Port manual das camadas não-UI do CountriesSwiftUI.
//! Mantém nomes camelCase e a estrutura de pastas do Swift.

#![allow(non_snake_case)]

pub mod Foundation;

pub mod Core {
    pub mod AppState;
    pub mod DeepLinksHandler;
    pub mod PushNotificationsHandler;
    pub mod SystemEventsHandler;
}

pub mod DependencyInjection {
    pub mod AppEnvironment;
    pub mod DIContainer;
}

pub mod Interactors {
    pub mod CountriesInteractor;
    pub mod ImagesInteractor;
    pub mod UserPermissionsInteractor;
}

pub mod Repositories {
    pub mod Database {
        pub mod CountriesDBRepository;
        pub mod ModelContainer;
    }
    pub mod Models {
        pub mod ApiModel;
        pub mod AppSchema;
        pub mod DBModel;
    }
    pub mod WebAPI {
        pub mod CountriesWebRepository;
        pub mod ImagesWebRepository;
        pub mod PushTokenWebRepository;
        pub mod WebRepository;
    }
}

pub mod Utilities {
    pub mod Helpers;
}
