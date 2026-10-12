import Foundation
import UserNotifications

/// Opens a notification sealed by the machine with the phone's notification
/// key and shows the real text. dent-push only ever carried the sealed
/// bytes. If anything is off, the generic text dent-push attached stays.
final class NotificationService: UNNotificationServiceExtension {
    override func didReceive(_ request: UNNotificationRequest,
                             withContentHandler handler: @escaping (UNNotificationContent) -> Void) {
        guard let content = request.content.mutableCopy() as? UNMutableNotificationContent else {
            handler(request.content)
            return
        }
        if let sealed = (request.content.userInfo["e"] as? String).flatMap({ Data(base64Encoded: $0) }),
           let keyText = UserDefaults(suiteName: "group.dev.dent.spike07")?.string(forKey: "pushKey"),
           let key = Data(base64Encoded: keyText),
           let n = SealedNotice.open(sealed, key: key) {
            content.title = n.title
            content.body = n.body
            if let s = n.session { content.threadIdentifier = s }
        }
        handler(content)
    }
}
