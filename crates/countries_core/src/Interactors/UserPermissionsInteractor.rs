//
//  UserPermissionsInteractor.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use crate::Core::AppState::{AppState, PermissionStatus};
use motor::combine::Store;
use std::rc::Rc;

/// `enum Permission { case pushNotifications }`
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Permission {
    PushNotifications,
}

/// `extension Permission { enum Status }` — lives in AppState in the port
/// (`PermissionStatus`) because AppState references it.

/// `protocol SystemNotificationsSettings { var authorizationStatus }`
pub trait SystemNotificationsSettings {
    fn authorizationStatus(&self) -> UNAuthorizationStatus;
}

/// `protocol SystemNotificationsCenter`
pub trait SystemNotificationsCenter {
    fn currentSettings(&self) -> SystemNotificationsSettingsBox;
    fn requestAuthorization(
        &self,
        options: Vec<UNAuthorizationOptions>,
    ) -> Result<bool, motor::loadable::LoadError>;
}

/// `UNAuthorizationStatus` fake
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UNAuthorizationStatus {
    NotDetermined,
    Denied,
    Authorized,
    Provisional,
    Ephemeral,
}

/// `UNAuthorizationOptions` fake — `[.alert, .sound]`
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UNAuthorizationOptions {
    Alert,
    Sound,
    Badge,
}

/// the `any SystemNotificationsSettings` existential (non-Sized methods)
pub type SystemNotificationsSettingsBox = Rc<dyn SystemNotificationsSettings>;

/// `extension UNAuthorizationStatus { var map: Permission.Status }`
pub trait AuthorizationStatusMap {
    fn map(&self) -> PermissionStatus;
}

impl AuthorizationStatusMap for UNAuthorizationStatus {
    fn map(&self) -> PermissionStatus {
        match self {
            UNAuthorizationStatus::Denied => PermissionStatus::Denied,
            UNAuthorizationStatus::Authorized => PermissionStatus::Granted,
            UNAuthorizationStatus::NotDetermined
            | UNAuthorizationStatus::Provisional
            | UNAuthorizationStatus::Ephemeral => PermissionStatus::NotRequested,
        }
    }
}

/// Fake `UNUserNotificationCenter.current()` — granted by default (the demo
/// needs the push flow working headless).
pub struct UNUserNotificationCenter;

impl UNUserNotificationCenter {
    pub fn current() -> SystemNotificationsCenterBox {
        Rc::new(GrantedCenter)
    }
}

pub type SystemNotificationsCenterBox = Rc<dyn SystemNotificationsCenter>;

struct GrantedCenter;

struct GrantedSettings;

impl SystemNotificationsSettings for GrantedSettings {
    fn authorizationStatus(&self) -> UNAuthorizationStatus {
        UNAuthorizationStatus::Authorized
    }
}

impl SystemNotificationsCenter for GrantedCenter {
    fn currentSettings(&self) -> SystemNotificationsSettingsBox {
        Rc::new(GrantedSettings)
    }
    fn requestAuthorization(
        &self,
        _options: Vec<UNAuthorizationOptions>,
    ) -> Result<bool, motor::loadable::LoadError> {
        Ok(true)
    }
}

// MARK: - RealUserPermissionsInteractor

pub struct RealUserPermissionsInteractor {
    pub appState: Store<AppState>,
    pub openAppSettings: Rc<dyn Fn()>,
    pub notificationCenter: SystemNotificationsCenterBox,
}

impl RealUserPermissionsInteractor {
    pub fn new(
        appState: Store<AppState>,
        openAppSettings: Rc<dyn Fn()>,
    ) -> Self {
        RealUserPermissionsInteractor {
            appState,
            openAppSettings,
            notificationCenter: UNUserNotificationCenter::current(),
        }
    }

    /// `AppState.permissionKeyPath(for:)` — the Swift keypath becomes a
    /// getter/setter pair over the store.
    fn permissionKeyPath(&self, permission: &Permission) -> PermissionStatus {
        self.appState.value().permissionStatus(permission)
    }

    fn setPermissionKeyPath(&self, permission: &Permission, status: PermissionStatus) {
        self.appState.update(|state| state.setPermissionStatus(permission, status));
    }

    async fn pushNotificationsPermissionStatus(&self) -> PermissionStatus {
        self.notificationCenter
            .currentSettings()
            .authorizationStatus()
            .map()
    }

    async fn requestPushNotificationsPermission(&self) {
        let center = self.notificationCenter.clone();
        let isGranted = center
            .requestAuthorization(vec![
                UNAuthorizationOptions::Alert,
                UNAuthorizationOptions::Sound,
            ])
            .unwrap_or(false);
        self.setPermissionKeyPath(
            &Permission::PushNotifications,
            if isGranted { PermissionStatus::Granted } else { PermissionStatus::Denied },
        );
    }
}

impl UserPermissionsInteractor for RealUserPermissionsInteractor {
    fn resolveStatus(&self, permission: Permission) {
        let currentStatus = self.permissionKeyPath(&permission);
        if currentStatus != PermissionStatus::Unknown {
            return;
        }
        // Task { @MainActor in … } — everything is synchronous in the single-thread fake
        let status = motor::block_on(self.pushNotificationsPermissionStatus());
        self.setPermissionKeyPath(&permission, status);
    }

    fn request(&self, permission: Permission) {
        let currentStatus = self.permissionKeyPath(&permission);
        if currentStatus == PermissionStatus::Denied {
            (self.openAppSettings)();
            return;
        }
        motor::block_on(self.requestPushNotificationsPermission());
    }
}

/// `protocol UserPermissionsInteractor: AnyObject`
pub trait UserPermissionsInteractor {
    fn resolveStatus(&self, permission: Permission);
    fn request(&self, permission: Permission);
}

pub struct StubUserPermissionsInteractor;

impl UserPermissionsInteractor for StubUserPermissionsInteractor {
    fn resolveStatus(&self, _permission: Permission) {}
    fn request(&self, _permission: Permission) {}
}
