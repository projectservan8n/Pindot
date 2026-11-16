// wksnap: load a page in the system WebKit (the engine Tauri uses on macOS)
// and save a PNG. Headless Chrome is NOT what Pindot runs in on a Mac.
// usage: wksnap [flags] <url> <out.png> <width> <height> [waitMs] [inject.js] [after.js]
//   inject.js runs at document start (stubs, fake data); after.js runs after
//   load, before the wait. Prints window.__logs and any JS errors.
// flags: --scheme dark|light   force the appearance (prefers-color-scheme follows it)
//        --frames N            take N snapshots (out-01.png, out-02.png, ...) for animation strips
//        --interval MS         time between frames (default 120)
//        --scale S             snapshot width scale (default 1)
import Cocoa
import WebKit

var argv = Array(CommandLine.arguments.dropFirst())
var scheme: String? = nil
var frames = 1
var interval = 120
func takeFlag(_ name: String) -> String? {
  if let i = argv.firstIndex(of: name), i + 1 < argv.count { let v = argv[i + 1]; argv.removeSubrange(i...(i + 1)); return v }
  return nil
}
scheme = takeFlag("--scheme")
if let f = takeFlag("--frames") { frames = max(1, Int(f) ?? 1) }
if let f = takeFlag("--interval") { interval = Int(f) ?? 120 }
let args = [CommandLine.arguments[0]] + argv
guard args.count >= 5 else { print("usage: wksnap url out.png width height [waitMs] [inject.js] [after.js]"); exit(2) }
let url = URL(string: args[1])!
let out = args[2]
let w = Double(args[3])!, h = Double(args[4])!
let waitMs = args.count > 5 ? Int(args[5])! : 800
let inject: String? = (args.count > 6 && !args[6].isEmpty) ? try! String(contentsOfFile: args[6], encoding: .utf8) : nil
let after: String? = (args.count > 7 && !args[7].isEmpty) ? try! String(contentsOfFile: args[7], encoding: .utf8) : nil
let errHook = "window.__logs = window.__logs || []; window.addEventListener('error', function (e) { window.__logs.push('ERROR ' + e.message + ' @' + (e.filename || '') + ':' + e.lineno); });"

class Delegate: NSObject, WKNavigationDelegate {
  func shoot(_ webView: WKWebView, _ n: Int) {
    webView.takeSnapshot(with: WKSnapshotConfiguration()) { img, err in
      guard let img = img, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
            let png = rep.representation(using: .png, properties: [:]) else {
        print("snapshot failed: \(String(describing: err))"); exit(1)
      }
      var path = out
      if frames > 1 { path = out.replacingOccurrences(of: ".png", with: String(format: "-%02d.png", n)) }
      try! png.write(to: URL(fileURLWithPath: path))
      if n >= frames {
        webView.evaluateJavaScript("JSON.stringify(window.__logs || [])") { r, _ in print("logs: \(r ?? "nil")"); exit(0) }
      } else {
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(interval)) { self.shoot(webView, n + 1) }
      }
    }
  }
  func snap(_ webView: WKWebView) {
    DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(waitMs)) { self.shoot(webView, 1) }
  }
  func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
    if let after = after {
      webView.evaluateJavaScript(after) { _, e in
        if let e = e { print("after.js error: \(e)") }
        self.snap(webView)
      }
    } else { snap(webView) }
  }
  func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { print("fail: \(error)"); exit(1) }
  func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { print("fail: \(error)"); exit(1) }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let cfg = WKWebViewConfiguration()
cfg.userContentController.addUserScript(WKUserScript(source: errHook, injectionTime: .atDocumentStart, forMainFrameOnly: true))
if let inject = inject {
  cfg.userContentController.addUserScript(WKUserScript(source: inject, injectionTime: .atDocumentStart, forMainFrameOnly: true))
}
let appearance: NSAppearance? = scheme == "dark" ? NSAppearance(named: .darkAqua) : scheme == "light" ? NSAppearance(named: .aqua) : nil
let wv = WKWebView(frame: NSRect(x: 0, y: 0, width: w, height: h), configuration: cfg)
let d = Delegate()
wv.navigationDelegate = d
// A real window in the bottom-left corner of the screen for the few seconds
// the run takes: WebKit stops animation frames in a hidden or off-screen
// window, so parked off screen the page never animates. It ignores the mouse.
let win = NSWindow(contentRect: NSRect(x: 0, y: 0, width: w, height: h), styleMask: [.borderless], backing: .buffered, defer: false)
if let a = appearance { win.appearance = a; wv.appearance = a }
win.contentView = wv
win.level = .floating
win.ignoresMouseEvents = true
win.orderFrontRegardless()
wv.load(URLRequest(url: url))
DispatchQueue.main.asyncAfter(deadline: .now() + 40 + Double(frames * (interval + 600)) / 1000) { print("timeout"); exit(1) }
app.run()
