import Foundation

enum DesktopFiles {
    static let limit = 8 * 1024 * 1024
    static var defaultFolder: URL {
        FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("VniLink", isDirectory: true)
    }
    static func read(_ url: URL) throws -> Data {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let values = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
        guard values.isRegularFile == true else { throw Failure.regularFileRequired }
        guard let size = values.fileSize, size <= limit else { throw Failure.tooLarge }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        // Also bound the read if the file grows after its metadata was checked.
        let data = try handle.read(upToCount: limit + 1) ?? Data()
        guard data.count <= limit else { throw Failure.tooLarge }
        return data
    }
    enum Failure: LocalizedError {
        case tooLarge, regularFileRequired
        var errorDescription: String? {
            switch self {
            case .tooLarge: return "Files must be 8 MiB or smaller."
            case .regularFileRequired: return "Choose a file to send. Folders are not supported yet."
            }
        }
    }
}

/// Changes applied from the remote pasteboard must not echo back as local copies.
struct ClipboardChanges {
    private var observed: Int
    init(changeCount: Int) { observed = changeCount }
    mutating func changed(_ changeCount: Int) -> Bool {
        defer { observed = changeCount }
        return observed != changeCount
    }
    mutating func applied(_ changeCount: Int) { observed = changeCount }
}
