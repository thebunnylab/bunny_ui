//
//  AppState.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use crate::Interactors::UserPermissionsInteractor::Permission;

/// `struct AppState: Equatable`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppState {
    pub routing: ViewRouting,
    pub system: System,
    pub permissions: Permissions,
}

/// `extension AppState { struct ViewRouting }` — os `CountriesList.Routing` /
/// `CountryDetails.Routing` das views vivem aqui (como no Swift).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewRouting {
    pub countriesList: CountriesListRouting,
    pub countryDetails: CountryDetailsRouting,
}

/// `CountriesList.Routing { var countryCode: String? }`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CountriesListRouting {
    pub countryCode: Option<String>,
}

/// `CountryDetails.Routing { var detailsSheet: Bool = false }`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CountryDetailsRouting {
    pub detailsSheet: bool,
}

/// `extension AppState { struct System }`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct System {
    pub isActive: bool,
    pub keyboardHeight: f64,
}

/// `extension Permission { enum Status }`
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PermissionStatus {
    Unknown,
    NotRequested,
    Granted,
    Denied,
}

impl Default for PermissionStatus {
    fn default() -> Self {
        PermissionStatus::Unknown
    }
}

/// `extension AppState { struct Permissions }`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Permissions {
    pub push: PermissionStatus,
}

impl AppState {
    /// `static func permissionKeyPath(for:) -> WritableKeyPath` — keypath não
    /// existe em Rust; o par de accessors cumpre o mesmo papel.
    pub fn permissionStatus(&self, _permission: &Permission) -> PermissionStatus {
        self.permissions.push
    }

    pub fn setPermissionStatus(&mut self, _permission: &Permission, status: PermissionStatus) {
        self.permissions.push = status;
    }
}
