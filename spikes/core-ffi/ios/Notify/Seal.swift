import CryptoKit
import Foundation

/// A notice sealed by a machine for this phone: ChaCha20-Poly1305, in
/// CryptoKit's combined form (nonce, ciphertext, tag), exactly as the core's
/// `push::seal` writes it. Compiled into the app (for checks) and the
/// notification extension (to open real pushes).
struct SealedNotice: Decodable {
    let title: String
    let body: String
    let session: String?

    static func open(_ sealed: Data, key: Data) -> SealedNotice? {
        guard let box = try? ChaChaPoly.SealedBox(combined: sealed),
              let plain = try? ChaChaPoly.open(box, using: SymmetricKey(data: key)) else { return nil }
        return try? JSONDecoder().decode(SealedNotice.self, from: plain)
    }
}
