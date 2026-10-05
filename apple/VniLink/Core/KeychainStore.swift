import Foundation
import Security
import VnilinkCore

/// One opaque Rust installation record. Swift never interprets its bytes.
final class KeychainStore: CredentialStore, @unchecked Sendable {
    private let service = "com.vnilink.installation"
    private let account = "primary"
    private var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service, kSecAttrAccount as String: account,
         kSecAttrSynchronizable as String: false]
    }
    func load() throws -> Data? {
        var request = query
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        try check(status)
        guard let data = result as? Data else { throw FfiError.StoreUnavailable }
        return data
    }
    func save(record: Data) throws {
        let status = SecItemUpdate(query as CFDictionary,
                                  [kSecValueData as String: record] as CFDictionary)
        if status == errSecItemNotFound {
            var item = query
            item[kSecValueData as String] = record
            item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            try check(SecItemAdd(item as CFDictionary, nil))
        } else { try check(status) }
    }
    private func check(_ status: OSStatus) throws {
        if status == errSecSuccess { return }
        if status == errSecInteractionNotAllowed || status == errSecAuthFailed {
            throw FfiError.StoreLocked
        }
        throw FfiError.StoreUnavailable
    }
}
