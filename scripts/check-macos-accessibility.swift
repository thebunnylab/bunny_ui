#!/usr/bin/env swift
// External AX protocol witness. This does not establish human VoiceOver acceptance.
import AppKit
import ApplicationServices
import Foundation

struct ProbeFailure: Error, CustomStringConvertible {
    let description: String
}
func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ProbeFailure(description: message) }
}
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}
func scalar(_ element: AXUIElement, _ name: String) -> Double? {
    (attribute(element, name) as? NSNumber)?.doubleValue
}
func find(_ root: AXUIElement, _ label: String, depth: Int = 0) -> AXUIElement? {
    guard depth < 12 else { return nil }
    for name in [kAXTitleAttribute, kAXDescriptionAttribute] {
        if attribute(root, name) as? String == label { return root }
    }
    for name in [kAXChildrenAttribute, kAXWindowsAttribute] {
        for child in (attribute(root, name) as? [AXUIElement] ?? []) {
            if let found = find(child, label, depth: depth + 1) { return found }
        }
    }
    return nil
}
func wait(_ message: String, _ check: () -> Bool) throws {
    let deadline = Date().addingTimeInterval(10)
    repeat {
        if check() { return }
        CFRunLoopRunInMode(.defaultMode, 0.03, true)
    } while Date() < deadline
    throw ProbeFailure(description: message)
}
final class Notifications {
    var values = 0
}

if !AXIsProcessTrusted() {
    print("BLOCKED: AXIsProcessTrusted=false; external AX inspection requires user-granted Accessibility permission.")
    exit(77)
}
guard CommandLine.arguments.count == 2 else {
    fputs("Usage: swift scripts/check-macos-accessibility.swift <accessibility_native-executable>\n", stderr)
    exit(2)
}
let fixture = Process()
fixture.executableURL = URL(fileURLWithPath: CommandLine.arguments[1])
fixture.arguments = ["--external-ax-fixture"]
do {
    try fixture.run()
    defer {
        if fixture.isRunning { fixture.terminate() }
        fixture.waitUntilExit()
    }
    let root = AXUIElementCreateApplication(fixture.processIdentifier)
    var range: AXUIElement?
    try wait("labeled slider was not exposed") { range = find(root, "Refresh rate"); return range != nil }
    guard let range else { throw ProbeFailure(description: "missing range") }
    try require(attribute(range, kAXRoleAttribute) as? String == kAXSliderRole, "incorrect AX role")
    try require(scalar(range, kAXMinValueAttribute) == 0 && scalar(range, kAXMaxValueAttribute) == 1, "incorrect AX bounds")
    var observer: AXObserver?
    let notifications = Notifications()
    let status = AXObserverCreate(fixture.processIdentifier, { _, _, name, context in
        guard let context else { return }
        if name as String == kAXValueChangedNotification {
            Unmanaged<Notifications>.fromOpaque(context).takeUnretainedValue().values += 1
        }
    }, &observer)
    try require(status == .success, "AXObserverCreate failed: \(status.rawValue)")
    guard let observer else { throw ProbeFailure(description: "missing observer") }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), AXObserverGetRunLoopSource(observer), .defaultMode)
    try require(AXObserverAddNotification(observer, range, kAXValueChangedNotification as CFString,
        Unmanaged.passUnretained(notifications).toOpaque()) == .success, "value subscription failed")
    for (action, values) in [(kAXIncrementAction, [0.3, 0.6, 0.9, 1.0, 1.0]),
                             (kAXDecrementAction, [0.9, 0.6, 0.3, 0.0, 0.0])] {
        for expected in values {
            try require(AXUIElementPerformAction(range, action as CFString) == .success, "range adjustment failed")
            try wait("incorrect value after \(action)") { abs((scalar(range, kAXValueAttribute) ?? -10) - expected) < 1e-12 }
        }
    }
    try require(AXUIElementSetAttributeValue(range, kAXValueAttribute as CFString, NSNumber(value: 0.2)) == .success, "numeric set failed")
    try wait("numeric set did not snap") { abs((scalar(range, kAXValueAttribute) ?? -10) - 0.3) < 1e-12 }
    for invalid in [Double.nan, Double.infinity, -0.1, 1.1] {
        _ = AXUIElementSetAttributeValue(range, kAXValueAttribute as CFString, NSNumber(value: invalid))
        try require(abs((scalar(range, kAXValueAttribute) ?? -10) - 0.3) < 1e-12, "invalid set changed the value")
    }
    try require(AXUIElementSetAttributeValue(range, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success, "focus failed")
    try wait("range not focused") { scalar(range, kAXFocusedAttribute) == 1 }
    try wait("numeric change notification missing") { notifications.values > 0 }
    guard let toggle = find(root, "Toggle controls"), let modal = find(root, "Open modal") else {
        throw ProbeFailure(description: "fixture actions missing")
    }
    try require(AXUIElementPerformAction(toggle, kAXPressAction as CFString) == .success, "disable action failed")
    try wait("range not disabled") { scalar(range, kAXEnabledAttribute) == 0 }
    _ = AXUIElementPerformAction(range, kAXIncrementAction as CFString)
    _ = AXUIElementSetAttributeValue(range, kAXValueAttribute as CFString, NSNumber(value: 1.0))
    try require(abs((scalar(range, kAXValueAttribute) ?? -10) - 0.3) < 1e-12, "disabled range changed")
    try require(AXUIElementPerformAction(toggle, kAXPressAction as CFString) == .success, "enable action failed")
    try require(AXUIElementPerformAction(modal, kAXPressAction as CFString) == .success, "modal action failed")
    try wait("modal did not exclude background range") { find(root, "Refresh rate") == nil }
    try require(AXUIElementPerformAction(range, kAXIncrementAction as CFString) != .success, "retired AX range accepted adjustment")
    print("PASS: external AX range label, bounds, numeric actions, focus, notifications, disabled state and modal retirement; human VoiceOver acceptance remains separate.")
} catch {
    fputs("FAIL: \(error)\n", stderr)
    exit(1)
}
