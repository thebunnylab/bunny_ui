//
//  DeepLinksHandler.swift
//  CountriesSwiftUI
//
//  Created by Alexey Naumov on 26.04.2020.
//  Copyright © 2020 Alexey Naumov. All rights reserved.
//
//

#![allow(non_snake_case)]

use crate::Core::AppState::ViewRouting;
use crate::DependencyInjection::DIContainer::DIContainer;
use crate::Foundation::{URL, URLComponents};

/// `enum DeepLink: Equatable`
#[derive(Clone, Debug, PartialEq)]
pub enum DeepLink {
    ShowCountryFlag { alpha3Code: String },
}

impl DeepLink {
    /// `init?(url: URL)` — the failable init becomes an `Option` factory.
    pub fn new(url: &URL) -> Option<DeepLink> {
        let Some(components) = URLComponents::new(url, true) else { return None };
        if components.host.as_deref() != Some("www.example.com") {
            return None;
        }
        let Some(query) = components.queryItems else { return None };
        if let Some(item) = query.iter().find(|item| item.name == "alpha3code") {
            if let Some(alpha3Code) = &item.value {
                return Some(DeepLink::ShowCountryFlag { alpha3Code: alpha3Code.clone() });
            }
        }
        None
    }
}

// MARK: - DeepLinksHandler

/// `@MainActor protocol DeepLinksHandler`
pub trait DeepLinksHandler {
    fn open(&self, deepLink: DeepLink);
}

pub struct RealDeepLinksHandler {
    container: DIContainer,
}

impl RealDeepLinksHandler {
    /// `init(container: DIContainer)`
    pub fn new(container: DIContainer) -> Self {
        RealDeepLinksHandler { container }
    }
}

impl DeepLinksHandler for RealDeepLinksHandler {
    fn open(&self, deepLink: DeepLink) {
        match deepLink {
            DeepLink::ShowCountryFlag { alpha3Code } => {
                let routeToDestination = {
                    let appState = self.container.appState.clone();
                    move || {
                        appState.update(|state| {
                            state.routing.countriesList.countryCode =
                                Some(alpha3Code.clone());
                            state.routing.countryDetails.detailsSheet = true;
                        })
                    }
                };
                /*
                 SwiftUI is unable to perform complex navigation involving
                 simultaneous dismissal or older screens and presenting new ones.
                 A work around is to perform the navigation in two steps:
                 */
                let defaultRouting = ViewRouting::default();
                if self.container.appState.value().routing != defaultRouting {
                    self.container.appState.update(|state| state.routing = defaultRouting);
                    // DispatchQueue.main.asyncAfter(deadline:) — the fake is
                    // synchronous: routes right away, in sequence.
                    routeToDestination();
                } else {
                    routeToDestination();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Core::AppState::AppState;
    use motor::combine::Store;

    fn container() -> DIContainer {
        DIContainer::new(Store::new(AppState::default()), crate::DependencyInjection::DIContainer::Interactors::stub())
    }

    #[test]
    fn parsesDeepLinkFromURL() {
        let url = URL::new("https://www.example.com/?alpha3code=USA".into()).unwrap();
        assert_eq!(
            DeepLink::new(&url),
            Some(DeepLink::ShowCountryFlag { alpha3Code: "USA".into() })
        );
    }

    #[test]
    fn rejectsForeignHostsAndMissingQuery() {
        assert!(DeepLink::new(&URL::new("https://other.com/?alpha3code=USA".into()).unwrap()).is_none());
        assert!(DeepLink::new(&URL::new("https://www.example.com/".into()).unwrap()).is_none());
    }

    #[test]
    fn openRoutesTwoStepOrDirect() {
        let container = container();
        let handler = RealDeepLinksHandler::new(container.clone());
        handler.open(DeepLink::ShowCountryFlag { alpha3Code: "USA".into() });
        let routing = container.appState.value().routing;
        assert_eq!(routing.countriesList.countryCode.as_deref(), Some("USA"));
        assert!(routing.countryDetails.detailsSheet);
        // the second call hits the workaround's "reset then route" path
        handler.open(DeepLink::ShowCountryFlag { alpha3Code: "GEO".into() });
        assert_eq!(container.appState.value().routing.countriesList.countryCode.as_deref(), Some("GEO"));
    }
}
