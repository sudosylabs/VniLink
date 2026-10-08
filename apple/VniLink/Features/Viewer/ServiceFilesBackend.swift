import Foundation
import VnilinkCore

/// One file window's service and queue. Every remote action uses its captured
/// connection epoch; this adapter never falls back to the global job engine.
final class ServiceFilesBackend: FilesBackend, @unchecked Sendable {
    private let core: VnilinkCore
    private let peerId: String
    private let lock = NSLock()
    private let opening = NSLock()
    private var owner: FilesService?
    private var closed = false

    init(core: VnilinkCore, peerId: String) { self.core = core; self.peerId = peerId }
    private func current() throws -> FilesService {
        lock.lock(); defer { lock.unlock() }
        guard !closed, let owner else { throw FfiError.NoSession }
        return owner
    }
    private func files(_ id: UInt64, _ generation: UInt64) throws -> FilesService {
        // Serialize admission off the UI thread, without blocking close on it.
        opening.lock(); defer { opening.unlock() }
        lock.lock()
        if closed { lock.unlock(); throw FfiError.NoSession }
        let existing = owner
        lock.unlock()
        if let existing {
            let info = existing.info()
            if info.connectionId == id && info.generation == generation && !existing.isCancelled() { return existing }
        }
        let session = try core.controllerSession(sessionId: id)
        guard session.phase == .active, session.generation == generation, session.peerId == peerId else { throw FfiError.StaleSession }
        if let existing, let info = try? core.controllerService(id: existing.info().id) {
            try core.closeControllerService(id: info.id, generation: info.generation)
        }
        let deadline = Date().addingTimeInterval(1)
        let info: FfiServiceInfo
        while true {
            do { info = try core.attachControllerService(connectionId: id, generation: generation, kind: .files); break }
            catch FfiError.Busy where Date() < deadline {
                // A window just closed; let its stream settle before reopening.
                lock.lock(); let ended = closed; lock.unlock()
                if ended { throw FfiError.NoSession }
                Thread.sleep(forTimeInterval: 0.01)
            }
        }
        let next: FilesService
        do { next = try core.openFilesService(id: info.id, generation: info.generation) }
        catch { try? core.closeControllerService(id: info.id, generation: info.generation); throw error }
        lock.lock()
        if closed { lock.unlock(); next.close(); throw FfiError.NoSession }
        owner = next
        lock.unlock()
        return next
    }
    func queueOwner() -> String? {
        lock.lock(); defer { lock.unlock() }
        guard !closed, let owner else { return nil }
        let info = owner.info()
        return "\(info.id):\(info.generation)"
    }
    func close() {
        lock.lock(); closed = true; let previous = owner; lock.unlock()
        previous?.close()
    }
    func fileRoots() throws -> [FfiFileRoot] { try core.fileRoots() }
    func listDir(root: UInt64, path: String, offset: UInt64, limit: UInt16) throws -> FfiDirPage { try core.listDir(root: root, path: path, offset: offset, limit: limit) }
    func createDir(root: UInt64, path: String, name: String) throws { try core.createDir(root: root, path: path, name: name) }
    func renameEntry(root: UInt64, path: String, name: String, expected: FfiEntryToken?, newName: String) throws { try core.renameEntry(root: root, path: path, name: name, expected: expected, newName: newName) }
    func fsList(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, offset: UInt64, limit: UInt16) throws -> FfiDirPage { try files(sessionId, generation).list(root: root, path: path, offset: offset, limit: limit) }
    func fsMkdir(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, name: String) throws { try files(sessionId, generation).mkdir(root: root, path: path, name: name) }
    func fsRename(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, name: String, expected: FfiEntryToken?, newName: String) throws { try files(sessionId, generation).rename(root: root, path: path, name: name, expected: expected, newName: newName) }
    func jobSubmitUpload(sessionId: UInt64, generation: UInt64, peerEndpointId: String, srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String) throws -> UInt64 {
        guard peerEndpointId == peerId else { throw FfiError.StaleSession }
        return try files(sessionId, generation).upload(srcRoot: srcRoot, srcRel: srcRel, dstRoot: dstRoot, dstRel: dstRel)
    }
    func jobSubmitDownload(sessionId: UInt64, generation: UInt64, peerEndpointId: String, srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String) throws -> UInt64 {
        guard peerEndpointId == peerId else { throw FfiError.StaleSession }
        return try files(sessionId, generation).download(srcRoot: srcRoot, srcRel: srcRel, dstRoot: dstRoot, dstRel: dstRel)
    }
    func jobSnapshot(id: UInt64) throws -> FfiJobSnapshot { try current().jobSnapshot(job: id) }
    func jobList() throws -> [FfiJobSnapshot] {
        lock.lock(); let previous = owner; let ended = closed; lock.unlock()
        guard !ended else { throw FfiError.NoSession }
        return try previous?.jobs() ?? []
    }
    func jobPoll(id: UInt64) throws -> FfiJobSnapshot { try current().jobPoll(job: id) }
    func jobCancel(id: UInt64) throws -> FfiJobSnapshot { try current().jobCancel(job: id) }
    func jobResume(id: UInt64) throws -> FfiJobSnapshot { try current().jobResume(job: id) }
    func jobResolve(id: UInt64, choice: FfiConflictChoice, applyToRemaining: Bool) throws -> FfiJobSnapshot { try current().jobResolve(job: id, choice: choice, applyToRemaining: applyToRemaining) }
    func jobForget(id: UInt64) throws { try current().jobForget(job: id) }
}
