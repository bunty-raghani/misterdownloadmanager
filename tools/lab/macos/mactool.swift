// What the macOS runner's scripts (tools/lab/macos/*.sh) ask of the window server: the displays
// and their modes, the windows of a process, a click or a key, the system's light or dark
// appearance. Built by lib.sh with the runner's own swiftc (Xcode's command line tools).
//
//   mactool displays                 every display: its mode (points, pixels), every mode it offers
//   mactool mode <w> <h> <pixel w>   the main display in that mode (HiDPI when pixel w = 2 w)
//   mactool windows [pid]            on-screen windows: id pid layer x y w h owner title (tabs)
//   mactool click <x> <y> [right]    a click at that point (screen points, top left 0,0)
//   mactool move <x> <y>             the pointer there
//   mactool key <code> [cmd]         a key press (virtual key code: 53 Escape, 36 Return)
//   mactool access                   what this process may do (screen capture, events, AX)
//   mactool activate <pid>           that app in front
//   mactool appearance <dark|light>  the system's appearance (the global preference, then the
//                                    notification apps listen for), when osascript can't
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

func say(_ s: String) {
    print(s)
    fflush(stdout)
}

func modes(_ id: CGDirectDisplayID) -> [CGDisplayMode] {
    let opts = [kCGDisplayShowDuplicateLowResolutionModes as String: true] as CFDictionary
    return (CGDisplayCopyAllDisplayModes(id, opts) as? [CGDisplayMode]) ?? []
}

func describe(_ m: CGDisplayMode) -> String {
    return "\(m.width)x\(m.height) pixels=\(m.pixelWidth)x\(m.pixelHeight) hz=\(m.refreshRate) gui=\(m.isUsableForDesktopGUI())"
}

func displays() {
    var count: UInt32 = 0
    CGGetActiveDisplayList(0, nil, &count)
    var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
    CGGetActiveDisplayList(count, &ids, &count)
    for id in ids {
        let b = CGDisplayBounds(id)
        let mode = CGDisplayCopyDisplayMode(id).map(describe) ?? "?"
        say("display \(id) main=\(CGDisplayIsMain(id) != 0) now=\(mode) at=\(Int(b.origin.x)),\(Int(b.origin.y))")
        for m in modes(id) {
            say("  mode \(describe(m))")
        }
    }
    for s in NSScreen.screens {
        say("screen \(s.localizedName) frame=\(Int(s.frame.width))x\(Int(s.frame.height)) scale=\(s.backingScaleFactor)")
    }
}

func setMode(_ w: Int, _ h: Int, _ pw: Int) -> Int32 {
    let id = CGMainDisplayID()
    guard let m = modes(id).first(where: { $0.width == w && $0.height == h && $0.pixelWidth == pw }) else {
        say("no mode \(w)x\(h) with \(pw) pixels across")
        return 1
    }
    var cfg: CGDisplayConfigRef?
    guard CGBeginDisplayConfiguration(&cfg) == .success else {
        say("can't configure the display")
        return 1
    }
    CGConfigureDisplayWithDisplayMode(cfg, id, m, nil)
    let r = CGCompleteDisplayConfiguration(cfg, .permanently)
    say("mode \(describe(m)): \(r == .success ? "set" : "error \(r.rawValue)")")
    return r == .success ? 0 : 1
}

func windows(_ pid: Int32?) {
    let list = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]) ?? []
    for w in list {
        let owner = (w[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value ?? -1
        if let p = pid, p != owner { continue }
        let id = (w[kCGWindowNumber as String] as? NSNumber)?.intValue ?? 0
        let layer = (w[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0
        var r = CGRect.zero
        if let bd = w[kCGWindowBounds as String] as? NSDictionary {
            r = CGRect(dictionaryRepresentation: bd as CFDictionary) ?? .zero
        }
        let ownerName = (w[kCGWindowOwnerName as String] as? String) ?? ""
        let title = (w[kCGWindowName as String] as? String) ?? ""
        say("\(id)\t\(owner)\t\(layer)\t\(Int(r.origin.x))\t\(Int(r.origin.y))\t\(Int(r.width))\t\(Int(r.height))\t\(ownerName)\t\(title)")
    }
}

func post(_ e: CGEvent?) {
    e?.post(tap: .cghidEventTap)
}

func click(_ x: Double, _ y: Double, right: Bool) {
    let p = CGPoint(x: x, y: y)
    let src = CGEventSource(stateID: .hidSystemState)
    let button: CGMouseButton = right ? .right : .left
    post(CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: button))
    usleep(200_000)
    post(CGEvent(mouseEventSource: src, mouseType: right ? .rightMouseDown : .leftMouseDown, mouseCursorPosition: p, mouseButton: button))
    usleep(90_000)
    post(CGEvent(mouseEventSource: src, mouseType: right ? .rightMouseUp : .leftMouseUp, mouseCursorPosition: p, mouseButton: button))
}

func move(_ x: Double, _ y: Double) {
    let src = CGEventSource(stateID: .hidSystemState)
    post(CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: CGPoint(x: x, y: y), mouseButton: .left))
}

func key(_ code: UInt16, command: Bool) {
    let src = CGEventSource(stateID: .hidSystemState)
    let down = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(code), keyDown: true)
    let up = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(code), keyDown: false)
    if command {
        down?.flags = .maskCommand
        up?.flags = .maskCommand
    }
    post(down)
    usleep(60_000)
    post(up)
}

func appearance(dark: Bool) {
    let name = "AppleInterfaceStyle" as CFString
    CFPreferencesSetValue(name, dark ? ("Dark" as CFString) : nil, kCFPreferencesAnyApplication,
                          kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
    CFPreferencesSynchronize(kCFPreferencesAnyApplication, kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
    DistributedNotificationCenter.default().postNotificationName(
        NSNotification.Name("AppleInterfaceThemeChangedNotification"), object: nil, userInfo: nil,
        deliverImmediately: true)
    usleep(300_000)
}

let a = CommandLine.arguments
func arg(_ i: Int) -> String { return i < a.count ? a[i] : "" }
switch arg(1) {
case "displays":
    displays()
case "mode":
    exit(setMode(Int(arg(2)) ?? 0, Int(arg(3)) ?? 0, Int(arg(4)) ?? 0))
case "windows":
    windows(Int32(arg(2)))
case "click":
    click(Double(arg(2)) ?? 0, Double(arg(3)) ?? 0, right: arg(4) == "right")
case "move":
    move(Double(arg(2)) ?? 0, Double(arg(3)) ?? 0)
case "key":
    key(UInt16(arg(2)) ?? 53, command: arg(3) == "cmd")
case "access":
    say("screen capture: \(CGPreflightScreenCaptureAccess())")
    say("post events: \(CGPreflightPostEventAccess())")
    say("accessibility: \(AXIsProcessTrusted())")
case "activate":
    let ok = NSRunningApplication(processIdentifier: pid_t(Int32(arg(2)) ?? 0))?
        .activate(options: [.activateIgnoringOtherApps]) ?? false
    say("activate: \(ok)")
case "appearance":
    appearance(dark: arg(2) == "dark")
default:
    say("mactool displays | mode <w> <h> <pixel w> | windows [pid] | click <x> <y> [right] | move <x> <y> | key <code> [cmd] | access | activate <pid> | appearance <dark|light>")
    exit(2)
}
