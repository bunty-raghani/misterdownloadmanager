// Mister Download Manager's Safari extension: the native half. Safari's extension can't start a
// program of its own (no native messaging host manifests), it can only talk to this app
// extension. So this does nothing but pass each message on to the same helper the other browsers
// start (misterdownloadmanager-nmhost; the build puts a copy of it in this extension, signed to
// run inside its sandbox), and hand its answer back. All the rules live in the helper and in
// extension/ (one source for every browser): when the extension learns to say something new,
// nothing here changes.
//
// The sandbox (Safari only loads sandboxed extensions) is opened just as far as the helper needs:
// MDM's data folder (the channel to the app, Options' rules) and the loopback connection to the
// app (extension.entitlements).
//
// The helper speaks the browsers' native messaging protocol: a 32-bit little-endian length,
// then the JSON, both ways.

import Foundation
import SafariServices

final class SafariWebExtensionHandler: NSObject, NSExtensionRequestHandling {
    /// How long to wait for an answer: the helper starts the app and waits for it (30 s), as for
    /// the other browsers (extension/background.js HOST_WAIT).
    private static let wait: TimeInterval = 35

    func beginRequest(with context: NSExtensionContext) {
        let item = context.inputItems.first as? NSExtensionItem
        let message = item?.userInfo?[SFExtensionMessageKey]
        // The extension reads `ok`/`why` as it does from the other browsers' helper.
        let reply = Self.relay(message) ?? ["ok": false, "why": "host-failed", "error": "no answer from the helper"]
        let response = NSExtensionItem()
        response.userInfo = [SFExtensionMessageKey: reply]
        context.completeRequest(returningItems: [response], completionHandler: nil)
    }

    /// The helper, in this extension: X.appex/Contents/MacOS.
    private static var helper: URL {
        Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/misterdownloadmanager-nmhost")
    }

    /// MDM's data folder in the user's real home: the sandbox gives this process a home of its
    /// own, where the app's files are not (the helper reads MDM_DATA_DIR before anything).
    private static var dataFolder: String? {
        // (selftest.sh points it at a folder of its own.)
        if let given = ProcessInfo.processInfo.environment["MDM_DATA_DIR"] { return given }
        guard let entry = getpwuid(getuid()) else { return nil }
        return String(cString: entry.pointee.pw_dir) + "/Library/Application Support/misterdownloadmanager"
    }

    static func relay(_ message: Any?) -> [String: Any]? {
        guard let message, JSONSerialization.isValidJSONObject(message),
              let body = try? JSONSerialization.data(withJSONObject: message) else { return nil }
        let process = Process()
        process.executableURL = helper
        process.arguments = ["--safari"]
        if let folder = dataFolder {
            process.environment = ProcessInfo.processInfo.environment.merging(["MDM_DATA_DIR": folder]) { $1 }
        }
        let input = Pipe(), output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return ["ok": false, "why": "not-installed", "error": "\(error)"] }

        var length = UInt32(body.count).littleEndian
        var framed = Data(bytes: &length, count: 4)
        framed.append(body)
        input.fileHandleForWriting.write(framed)

        // Read the answer on another thread so a helper that never answers can be given up on.
        var answer: Data?
        let done = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            let out = output.fileHandleForReading
            let head = out.readData(ofLength: 4)
            if head.count == 4 {
                let n = Int(head.withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }.littleEndian)
                answer = out.readData(ofLength: n)
            }
            done.signal()
        }
        let finished = done.wait(timeout: .now() + wait) == .success
        try? input.fileHandleForWriting.close()
        if process.isRunning { process.terminate() }
        guard finished, let answer else { return ["ok": false, "why": "not-running"] }
        return (try? JSONSerialization.jsonObject(with: answer)) as? [String: Any]
    }
}
