// A throwaway window for harmless click tests. It shows one big button,
// logs every click (and Fn flag changes it receives) to a file, and quits
// by itself after --seconds. Spikes only ever click inside this window.
//
// Build: swiftc -O -o spikes/test-target/test-target spikes/test-target/main.swift
// Run:   spikes/test-target/test-target --log /tmp/dh-target.log --x 200 --y 200 --seconds 60 [--activate]

import Cocoa

let args = CommandLine.arguments
func arg(_ name: String, _ def: String) -> String {
    if let i = args.firstIndex(of: name), i + 1 < args.count { return args[i + 1] }
    return def
}
let logPath = arg("--log", "/tmp/deskhand-test-target.log")
let seconds = Double(arg("--seconds", "60")) ?? 60
let originX = CGFloat(Double(arg("--x", "200")) ?? 200)
let originY = CGFloat(Double(arg("--y", "200")) ?? 200)
let activate = args.contains("--activate")

func log(_ s: String) {
    let line = "\(Date().timeIntervalSince1970) \(s)\n"
    if let h = FileHandle(forWritingAtPath: logPath) {
        h.seekToEndOfFile()
        h.write(line.data(using: .utf8)!)
        h.closeFile()
    } else {
        FileManager.default.createFile(atPath: logPath, contents: line.data(using: .utf8))
    }
}

final class Target: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var clicks = 0
    var button: NSButton!

    func applicationDidFinishLaunching(_ notification: Notification) {
        let w: CGFloat = 440, h: CGFloat = 240
        // --x/--y are the window's top-left in global top-left-origin points.
        let primaryHeight = NSScreen.screens[0].frame.height
        let frame = NSRect(x: originX, y: primaryHeight - originY - h, width: w, height: h)
        window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "Deskhand Test Target"
        window.level = .floating
        window.isReleasedWhenClosed = false

        button = NSButton(title: "Deskhand Test Button", target: self, action: #selector(clicked))
        button.bezelStyle = .regularSquare
        button.font = NSFont.boldSystemFont(ofSize: 22)
        button.frame = NSRect(x: 50, y: 70, width: 340, height: 90)
        window.contentView!.addSubview(button)
        window.makeKeyAndOrderFront(nil)
        if activate { NSApp.activate(ignoringOtherApps: true) }

        let bf = window.convertToScreen(button.convert(button.bounds, to: nil))
        log("READY pid=\(ProcessInfo.processInfo.processIdentifier) button_global_topleft=\(Int(bf.origin.x)),\(Int(primaryHeight - bf.origin.y - bf.height)),\(Int(bf.width)),\(Int(bf.height))")

        NSEvent.addLocalMonitorForEvents(matching: [.flagsChanged]) { e in
            log("FLAGS keycode=\(e.keyCode) fn=\(e.modifierFlags.contains(.function))")
            return e
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
            log("EXIT timeout")
            NSApp.terminate(nil)
        }
    }

    @objc func clicked() {
        clicks += 1
        button.title = "Clicked \(clicks)x"
        log("CLICK \(clicks)")
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = Target()
app.delegate = delegate
app.run()
