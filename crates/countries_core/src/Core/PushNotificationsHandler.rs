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
/// — the `UNUserNotificationCenter.current().delegate = self` has no effect in
/// the headless fake (notifications arrive via `handleNotification`).
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
    /// `func handleNotification(userInfo:completionHandler:)` — the
    /// `[AnyHashable: Any]` becomes `serde_json::Value` (the nested subscripts
    /// `userInfo["aps"]["country"]` read the same as in Swift).
    pub fn handleNotification(&self, userInfo: &serde_json::Value, completionHandler: Rc<dyn Fn()>) {
        let Some(payload) = userInfo.get("aps") else {
            completionHandler();
            return;
        };
        let Some(countryCode) = payload.get("country").and_then(|value| value.as_str()) else {
            completionHandler();
            return;
        };
        // Task { @MainActor in … } — synchronous in the single-thread fake
        self.deepLinksHandler.open(DeepLink::ShowCountryFlag { alpha3Code: countryCode.to_string() });
        completionHandler();
    }
}
