import Foundation
import XCTest
import VnilinkCore
@testable import DesktopInteraction

private final class NativeFilesFixture: FilesService, @unchecked Sendable {
    let owner: FfiServiceInfo
    var ended = false
    var records: [FfiJobSnapshot] = []
    init(_ owner: FfiServiceInfo) { self.owner = owner; super.init(noPointer: .init()) }
    required init(unsafeFromRawPointer pointer: UnsafeMutableRawPointer) { fatalError("This fixture has no Rust pointer") }
    override func info() -> FfiServiceInfo { owner }
    override func isCancelled() -> Bool { ended }
    override func close() { ended = true }
    override func upload(srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String) throws -> UInt64 {
        guard !ended else { throw FfiError.NoSession }
        let id = owner.id * 100 + UInt64(records.count)
        records.append(FfiJobSnapshot(id: id, peer: owner.peerId, label: srcRel, direction: .upload, sender: true, state: .serving, totalBytes: 5, doneBytes: 0, filesTotal: 1, filesDone: 0, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: nil, error: nil))
        return id
    }
    override func jobs() throws -> [FfiJobSnapshot] { records }
    override func jobCancel(job: UInt64) throws -> FfiJobSnapshot {
        guard let record = records.first(where: { $0.id == job }) else { throw FfiError.InvalidSetting(message: "foreign job") }
        return record
    }
    override func list(root: UInt64, path: String, offset: UInt64, limit: UInt16) throws -> FfiDirPage {
        guard !ended else { throw FfiError.NoSession }
        return FfiDirPage(entries: [], total: 0, nextOffset: nil, truncated: false, nonUtf8Skipped: 0)
    }
}
private final class NativeCoreFixture: VnilinkCore, @unchecked Sendable {
    var generation: UInt64 = 2
    var next: UInt64 = 1
    var owners: [UInt64: NativeFilesFixture] = [:]
    var failOpen = false
    var busyAttempts = 0
    init() { super.init(noPointer: .init()) }
    required init(unsafeFromRawPointer pointer: UnsafeMutableRawPointer) { fatalError("This fixture has no Rust pointer") }
    override func controllerSession(sessionId: UInt64) throws -> FfiSessionInfo {
        FfiSessionInfo(id: sessionId, generation: generation, role: .controller, phase: .active, peerId: "peer-\(sessionId)", quality: nil, monitorWidth: 0, monitorHeight: 0)
    }
    override func attachControllerService(connectionId: UInt64, generation: UInt64, kind: FfiServiceKind) throws -> FfiServiceInfo {
        if busyAttempts > 0 { busyAttempts -= 1; throw FfiError.Busy }
        let info = FfiServiceInfo(id: next, connectionId: connectionId, generation: generation, peerId: "peer-\(connectionId)", kind: kind, phase: .active)
        next += 1; owners[info.id] = NativeFilesFixture(info); return info
    }
    override func openFilesService(id: UInt64, generation: UInt64) throws -> FilesService {
        if failOpen { throw FfiError.TransferDenied }
        return owners[id]!
    }
    override func controllerService(id: UInt64) throws -> FfiServiceInfo { owners[id]!.info() }
    override func closeControllerService(id: UInt64, generation: UInt64) throws { owners[id]?.close() }
}
final class ServiceFilesBackendTests: XCTestCase {
    func testWindowsRouteToDifferentOwnersAndCloseIndependently() throws {
        let core = NativeCoreFixture()
        let a = ServiceFilesBackend(core: core, peerId: "peer-9")
        let b = ServiceFilesBackend(core: core, peerId: "peer-10")
        let one = try a.jobSubmitUpload(sessionId: 9, generation: 2, peerEndpointId: "peer-9", srcRoot: 0, srcRel: "a", dstRoot: 0, dstRel: "")
        let two = try b.jobSubmitUpload(sessionId: 10, generation: 2, peerEndpointId: "peer-10", srcRoot: 0, srcRel: "b", dstRoot: 0, dstRel: "")
        XCTAssertEqual(try a.jobList().map(\.id), [one]); XCTAssertEqual(try b.jobList().map(\.id), [two])
        XCTAssertThrowsError(try a.jobCancel(id: two))
        XCTAssertEqual(core.owners.count, 2)
        a.close()
        XCTAssertTrue(core.owners[1]!.ended); XCTAssertFalse(core.owners[2]!.ended)
        XCTAssertThrowsError(try a.fsList(sessionId: 9, generation: 2, root: 0, path: "", offset: 0, limit: 1))
        XCTAssertEqual(try b.jobList().map(\.id), [two])
        _ = try b.fsList(sessionId: 10, generation: 2, root: 0, path: "", offset: 0, limit: 1)
        XCTAssertEqual(core.owners.count, 2, "Browsing must reuse this window's owner")
    }
    func testReconnectFencesOldEpochAndFailedOpenClosesItsOwner() throws {
        let core = NativeCoreFixture()
        let files = ServiceFilesBackend(core: core, peerId: "peer-9")
        _ = try files.fsList(sessionId: 9, generation: 2, root: 0, path: "", offset: 0, limit: 1)
        let old = files.queueOwner()
        core.generation = 3; core.owners[1]!.close(); core.busyAttempts = 1
        XCTAssertThrowsError(try files.fsList(sessionId: 9, generation: 2, root: 0, path: "", offset: 0, limit: 1))
        _ = try files.fsList(sessionId: 9, generation: 3, root: 0, path: "", offset: 0, limit: 1)
        XCTAssertNotEqual(files.queueOwner(), old)
        XCTAssertTrue(core.owners[1]!.ended)
        files.close()
        let failed = ServiceFilesBackend(core: core, peerId: "peer-10")
        core.failOpen = true
        XCTAssertThrowsError(try failed.fsList(sessionId: 10, generation: 3, root: 0, path: "", offset: 0, limit: 1))
        XCTAssertTrue(core.owners[3]!.ended)
    }
}
