// Prints the window number of the first on-screen window owned by a process,
// so screencapture -l can capture that window alone.
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments.dropFirst().first ?? "") ?? ProcessInfo.processInfo.processIdentifier
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerPID as String] as? Int32) == pid && (w[kCGWindowLayer as String] as? Int) == 0 {
    if let n = w[kCGWindowNumber as String] as? Int { print(n); exit(0) }
}
exit(1)
