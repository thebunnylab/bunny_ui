//
//  PushNotificationsHandler.swift
//  CountriesSwiftUI
//
//  Created by Alexey Naumov on 26.04.2020.
//  Copyright © 2020 Alexey Naumov. All rights reserved.
//
//

#![allow(non_snake_case)]

use crate::Core::DeepLinksHandler::{DeepLink, DeepLinksHandler};
use std::rc::Rc;

/// `protocol PushNotificationsHandler { }`
pub trait PushNotificationsHandler {}

/// `final class RealPushNotificationsHandler: NSObject, PushNotificationsHandler`
/// — o `UNUserNotificationCenter.current().delegate = self` não tem efeito no
/// fake headless (as notificações chegam via `handleNotification`).
pub struct RealPushNotificationsHandler {
    deepLinksHandler: Rc<dyn DeepLinksHandler>,
}

impl RealPushNotificationsHandler {
    /// `init(deepLinksHandler: DeepLinksHandler)`
    pub fn new(deepLinksHandler: Rc<dyn DeepLinksHandler>) -> Self {
        RealPushNotificationsHandler { deepLinksHandler }
    }
}

/// `final class RealPushNotificationsHandler: …, PushNotificationsHandler`
impl PushNotificationsHandler for RealPushNotificationsHandler {}

// MARK: - UNUserNotificationCenterDelegate

impl RealPushNotificationsHandler {
    /// `func handleNotification(userInfo:completionHandler:)` — o
    /// `[AnyHashable: Any]` vira `serde_json::Value` (os subscripts aninhados
    /// `userInfo["aps"]["country"]` leem igual ao Swift).
    pub fn handleNotification(&self, userInfo: &serde_json::Value, completionHandler: Rc<dyn Fn()>) {
        let Some(payload) = userInfo.get("aps") else {
            completionHandler();
            return;
        };
        let Some(countryCode) = payload.get("country").and_then(|value| value.as_str()) else {
            completionHandler();
            return;
        };
        // Task { @MainActor in … } — síncrono no fake single-thread
        self.deepLinksHandler.open(DeepLink::ShowCountryFlag { alpha3Code: countryCode.to_string() });
        completionHandler();
    }
}
