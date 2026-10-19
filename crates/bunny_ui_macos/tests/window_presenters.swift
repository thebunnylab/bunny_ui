// Reads only the windows owned by the supplied regression process.
import AppKit
import CoreGraphics

let pid = pid_t(CommandLine.arguments[1])!
let directory = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let stage = CommandLine.arguments[3]
guard let application = NSRunningApplication(processIdentifier: pid),
      application.localizedName == "window_presenters" else {
    fatalError("The target must be the window_presenters regression binary")
}
let windows = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as! [[String: Any]]
var result: [[String: Any]] = []
for window in windows where (window[kCGWindowOwnerPID as String] as? Int32) == pid && (window[kCGWindowLayer as String] as? Int) == 0 {
    guard let title = window[kCGWindowName as String] as? String,
          ["Presenter primary", "Presenter secondary"].contains(title),
          let id = window[kCGWindowNumber as String] as? Int else { continue }
    let path = directory.appendingPathComponent("\(stage)-\(id).png")
    let capture = Process()
    capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    capture.arguments = ["-x", "-o", "-l", String(id), path.path]
    try capture.run(); capture.waitUntilExit()
    guard capture.terminationStatus == 0,
          let bitmap = NSBitmapImageRep(data: try Data(contentsOf: path)),
          let color = bitmap.colorAt(x: bitmap.pixelsWide / 2, y: bitmap.pixelsHigh / 2)?.usingColorSpace(.sRGB) else {
        fatalError("Window capture unavailable; this host cannot qualify native pixels")
    }
    result.append(["title": title, "window": id, "path": path.path,
                   "rgb": [color.redComponent, color.greenComponent, color.blueComponent].map { Int(($0 * 255).rounded()) }])
}
let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
