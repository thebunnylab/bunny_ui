//
//  SystemEventsHandler.swift
//  CountriesSwiftUI
//
//  Created by Alexey Naumov on 27.10.2019.
//  Copyright © 2019 Alexey Naumov. All rights reserved.
//
//

#![allow(non_snake_case)]

use crate::Core::AppState::{AppState, PermissionStatus};
use crate::Core::DeepLinksHandler::{DeepLink, DeepLinksHandler};
use crate::Core::PushNotificationsHandler::PushNotificationsHandler;
use crate::DependencyInjection::DIContainer::DIContainer;
use crate::Foundation::{NotificationCenter, UIBackgroundFetchResult, UIOpenURLContext, URL};
use crate::Interactors::UserPermissionsInteractor::Permission;
use crate::Repositories::WebAPI::PushTokenWebRepository::PushTokenWebRepository;
use motor::cancel_bag::CancelBag;
use motor::loadable::LoadError;
use std::cell::Cell;
use std::rc::Rc;

/// `@MainActor protocol SystemEventsHandler`
pub trait SystemEventsHandler {
    fn sceneOpenURLContexts(&self, urlContexts: Vec<UIOpenURLContext>);
    fn sceneDidBecomeActive(&self);
    fn sceneWillResignActive(&self);
    fn handlePushRegistration(&self, result: Result<Vec<u8>, LoadError>);
    fn appDidReceiveRemoteNotification(&self, payload: &serde_json::Value) -> UIBackgroundFetchResult;
}

pub struct RealSystemEventsHandler {
    pub container: DIContainer,
    pub deepLinksHandler: Rc<dyn DeepLinksHandler>,
    pub pushNotificationsHandler: Rc<dyn PushNotificationsHandler>,
    pub pushTokenWebRepository: Rc<dyn PushTokenWebRepository>,
    cancelBag: CancelBag,
}

impl RealSystemEventsHandler {
    /// `init(container:deepLinksHandler:pushNotificationsHandler:pushTokenWebRepository:)`
    pub fn new(
        container: DIContainer,
        deepLinksHandler: Rc<dyn DeepLinksHandler>,
        pushNotificationsHandler: Rc<dyn PushNotificationsHandler>,
        pushTokenWebRepository: Rc<dyn PushTokenWebRepository>,
    ) -> Self {
        let handler = RealSystemEventsHandler {
            container,
            deepLinksHandler,
            pushNotificationsHandler,
            pushTokenWebRepository,
            cancelBag: CancelBag::new(),
        };
        handler.installKeyboardHeightObserver();
        handler.installPushNotificationsSubscriberOnLaunch();
        handler
    }

    /// `private func installKeyboardHeightObserver()`
    fn installKeyboardHeightObserver(&self) {
        let appState = self.container.appState.clone();
        let cancelBag = self.cancelBag.clone();
        NotificationCenter::default()
            .keyboardHeightPublisher()
            .sink(move |height| appState.update(|state| state.system.keyboardHeight = *height))
            .store_in(&cancelBag);
    }

    /// `private func installPushNotificationsSubscriberOnLaunch()` — o
    /// `.updates(for:).first(where: { $0 != .unknown })` vira subscribe com o
    /// flag `delivered` fazendo o papel do `.first(where:)`.
    fn installPushNotificationsSubscriberOnLaunch(&self) {
        let permissions = self.container.interactors.userPermissions.clone();
        let delivered = Rc::new(Cell::new(false));
        let appState = self.container.appState.clone();
        appState.subscribe(move |state: &AppState| {
            if delivered.get() {
                return;
            }
            let status = state.permissionStatus(&Permission::PushNotifications);
            if status != PermissionStatus::Unknown {
                delivered.set(true);
                if status == PermissionStatus::Granted {
                    // If the permission was granted on previous launch
                    // requesting the push token again:
                    permissions.request(Permission::PushNotifications);
                }
            }
        });
    }

    /// `private func handle(url: URL)`
    fn handle(&self, url: &URL) {
        let Some(deepLink) = DeepLink::new(url) else { return };
        self.deepLinksHandler.open(deepLink);
    }
}

impl SystemEventsHandler for RealSystemEventsHandler {
    fn sceneOpenURLContexts(&self, urlContexts: Vec<UIOpenURLContext>) {
        let Some(url) = urlContexts.first().map(|context| context.url.clone()) else { return };
        self.handle(&url);
    }

    fn sceneDidBecomeActive(&self) {
        self.container.appState.update(|state| state.system.isActive = true);
        self.container
            .interactors
            .userPermissions
            .resolveStatus(Permission::PushNotifications);
    }

    fn sceneWillResignActive(&self) {
        self.container.appState.update(|state| state.system.isActive = false);
    }

    fn handlePushRegistration(&self, result: Result<Vec<u8>, LoadError>) {
        let _ = result;
    }

    fn appDidReceiveRemoteNotification(
        &self,
        _payload: &serde_json::Value,
    ) -> UIBackgroundFetchResult {
        UIBackgroundFetchResult::NoData
    }
}
