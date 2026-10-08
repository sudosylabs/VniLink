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


/// Map a selected item into approved roots without reading its payload.
struct FilesSelection: Equatable {
    let root: UInt64
    let path: [String]
    let name: String
    static func locate(_ url: URL, roots: [(UInt64, URL)]) throws -> FilesSelection {
        let values = try url.resourceValues(forKeys: [.isRegularFileKey, .isDirectoryKey, .isSymbolicLinkKey])
        guard values.isSymbolicLink != true, values.isRegularFile == true || values.isDirectory == true else {
            throw Failure.unsupported
        }
        let item = url.standardizedFileURL.resolvingSymlinksInPath().pathComponents
        let candidates = roots.compactMap { id, root -> FilesSelection? in
            let base = root.standardizedFileURL.resolvingSymlinksInPath().pathComponents
            guard item.count > base.count, Array(item.prefix(base.count)) == base else { return nil }
            let relative = Array(item.dropFirst(base.count))
            return FilesSelection(root: id, path: Array(relative.dropLast()), name: relative.last!)
        }
        guard let selected = candidates.min(by: { $0.path.count < $1.path.count }) else { throw Failure.outsideRoots }
        return selected
    }
    enum Failure: LocalizedError {
        case unsupported, outsideRoots
        var errorDescription: String? {
            switch self {
            case .unsupported: return "Choose a regular file or folder. Links cannot be sent."
            case .outsideRoots: return "Choose an item inside a shared folder, or add its folder in Settings → Files."
            }
        }
    }
}
