// Independent pixels and session visibility; inspect only the owned fixture.
import AppKit
import CoreGraphics

let pid = pid_t(CommandLine.arguments[1])!
let directory = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let stage = CommandLine.arguments[3]
guard let app = NSRunningApplication(processIdentifier: pid), app.localizedName == "feed_receipts" else {
    fatalError("Expected owned feed_receipts fixture")
}
let session = CGSessionCopyCurrentDictionary() as? [String: Any]
let locked = session?["CGSSessionScreenIsLocked"] as? Bool
let windows = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as! [[String: Any]]
var result: [[String: Any]] = []
for window in windows where (window[kCGWindowOwnerPID as String] as? Int32) == pid
    && (window[kCGWindowLayer as String] as? Int) == 0
    && (window[kCGWindowName as String] as? String) == "Feed receipts" {
    guard let id = window[kCGWindowNumber as String] as? Int else { continue }
    let path = directory.appendingPathComponent("\(stage)-\(id).png")
    let capture = Process()
    capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    capture.arguments = ["-x", "-o", "-l", String(id), path.path]
    try capture.run(); capture.waitUntilExit()
    guard capture.terminationStatus == 0,
          let bitmap = NSBitmapImageRep(data: try Data(contentsOf: path)) else {
        fatalError("Window pixels unavailable")
    }
    var marker = 0
    var samples: [[Int]] = []
    for bit in 0..<8 {
        guard let color = bitmap.colorAt(x: bitmap.pixelsWide * (2 * bit + 1) / 16,
                                         y: bitmap.pixelsHigh / 2)?.usingColorSpace(.sRGB) else {
            fatalError("Cannot read frame-ID pixels")
        }
        let rgb = [color.redComponent, color.greenComponent, color.blueComponent].map { Int(($0 * 255).rounded()) }
        samples.append(rgb)
        if rgb.allSatisfy({ $0 >= 191 }) { marker |= 1 << bit }
        else if !rgb.allSatisfy({ $0 <= 64 }) { marker = -256; break }
    }
    let onScreen = window[kCGWindowIsOnscreen as String] as? Bool ?? false
    result.append(["window": id, "path": path.path, "marker": marker, "samples": samples,
                   "on_screen": onScreen, "session_unlocked": locked == false])
}
print(String(data: try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]), encoding: .utf8)!)
