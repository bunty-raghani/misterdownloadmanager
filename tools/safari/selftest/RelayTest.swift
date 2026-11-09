// selftest.sh's program: sends the helper a question through the extension's own relay, from
// inside the same sandbox, and checks the answer.
import Foundation

@main
struct RelayTest {
    static func main() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
        let reply = SafariWebExtensionHandler.relay(["rules": true, "userAgent": ua, "v": "0.0.0"])
        print(reply as Any)
        guard let reply, reply["ok"] as? Bool == true, reply["types"] is String else {
            print("FAIL: no rules came back")
            exit(1)
        }
        print("ok")
    }
}
