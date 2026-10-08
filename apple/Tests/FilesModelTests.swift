import Foundation
import XCTest
import VnilinkCore
@testable import DesktopInteraction

final class FakeFilesBackend: FilesBackend {
    var roots: [FfiFileRoot] = [FfiFileRoot(id: 0, path: "/local")]
    var localPages: [String: FfiDirPage] = [:]
    var remotePages: [String: FfiDirPage] = [:]
    var parkLocalPath: String?
    var parked = DispatchSemaphore(value: 0)
    var release = DispatchSemaphore(value: 0)
    var submitted: [(peer: String, srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String, upload: Bool)] = []
    var renamed: [(root: UInt64, path: String, name: String, token: FfiEntryToken?, newName: String, remote: Bool)] = []
    var resolved: [(id: UInt64, choice: FfiConflictChoice, apply: Bool)] = []
    var jobs: [UInt64: FfiJobSnapshot] = [:]
    var nextJob: UInt64 = 1
    var failList = false
    // Pagination + cancellation probes.
    var fullLocalNames: [String]?
    var parkPoll = false
    var pollParked = DispatchSemaphore(value: 0)
    var pollRelease = DispatchSemaphore(value: 0)
    var cancelSeen = false

    func fileRoots() throws -> [FfiFileRoot] { roots }

    func listDir(root: UInt64, path: String, offset: UInt64, limit: UInt16) throws -> FfiDirPage {
        if parkLocalPath == path {
            parked.signal()
            release.wait()
        }
        if failList { throw FfiError.NoSession }
        // When a full list is installed, honor offset/limit so pagination is
        // exercised; otherwise fall back to the stubbed single page.
        if let full = fullLocalNames, path == "" {
            let start = Int(offset)
            let end = min(full.count, start + Int(limit))
            let slice = Array(full[start..<end])
            let entries = slice.map { entry($0) }
            return FfiDirPage(entries: entries, total: UInt64(full.count), nextOffset: end < full.count ? UInt64(end) : nil, truncated: false, nonUtf8Skipped: 0)
        }
        return localPages[path] ?? FfiDirPage(entries: [], total: 0, nextOffset: nil, truncated: false, nonUtf8Skipped: 0)
    }

    func createDir(root: UInt64, path: String, name: String) throws {}
    func renameEntry(root: UInt64, path: String, name: String, expected: FfiEntryToken?, newName: String) throws {
        renamed.append((root, path, name, expected, newName, false))
    }
    func fsList(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, offset: UInt64, limit: UInt16) throws -> FfiDirPage {
        remotePages[path] ?? FfiDirPage(entries: [], total: 0, nextOffset: nil, truncated: false, nonUtf8Skipped: 0)
    }
    func fsMkdir(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, name: String) throws {}
    func fsRename(sessionId: UInt64, generation: UInt64, root: UInt64, path: String, name: String, expected: FfiEntryToken?, newName: String) throws {
        renamed.append((root, path, name, expected, newName, true))
    }
    func jobSubmitUpload(sessionId: UInt64, generation: UInt64, peerEndpointId: String, srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String) throws -> UInt64 {
        submitted.append((peerEndpointId, srcRoot, srcRel, dstRoot, dstRel, true))
        let id = nextJob; nextJob += 1
        jobs[id] = FfiJobSnapshot(id: id, peer: peerEndpointId, label: srcRel, direction: .upload, sender: true, state: .serving, totalBytes: 10, doneBytes: 0, filesTotal: 1, filesDone: 0, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: nil, error: nil)
        return id
    }
    func jobSubmitDownload(sessionId: UInt64, generation: UInt64, peerEndpointId: String, srcRoot: UInt64, srcRel: String, dstRoot: UInt64, dstRel: String) throws -> UInt64 {
        submitted.append((peerEndpointId, srcRoot, srcRel, dstRoot, dstRel, false))
        let id = nextJob; nextJob += 1
        jobs[id] = FfiJobSnapshot(id: id, peer: peerEndpointId, label: srcRel, direction: .download, sender: false, state: .transferring, totalBytes: 10, doneBytes: 0, filesTotal: 1, filesDone: 0, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: nil, error: nil)
        return id
    }
    func jobSnapshot(id: UInt64) throws -> FfiJobSnapshot {
        guard let job = jobs[id] else { throw FfiError.NoSession }
        return job
    }
    func jobList() throws -> [FfiJobSnapshot] { Array(jobs.values) }
    func jobPoll(id: UInt64) throws -> FfiJobSnapshot {
        if parkPoll {
            pollParked.signal()
            pollRelease.wait()
        }
        guard var job = jobs[id] else { throw FfiError.NoSession }
        if job.state == .transferring {
            job = FfiJobSnapshot(id: job.id, peer: job.peer, label: job.label, direction: job.direction, sender: job.sender, state: .complete, totalBytes: job.totalBytes, doneBytes: job.totalBytes, filesTotal: job.filesTotal, filesDone: job.filesTotal, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: nil, error: nil)
            jobs[id] = job
        }
        return job
    }
    func jobCancel(id: UInt64) throws -> FfiJobSnapshot {
        cancelSeen = true
        var job = jobs[id]!
        job = FfiJobSnapshot(id: job.id, peer: job.peer, label: job.label, direction: job.direction, sender: job.sender, state: .cancelled, totalBytes: job.totalBytes, doneBytes: job.doneBytes, filesTotal: job.filesTotal, filesDone: job.filesDone, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: nil, error: nil)
        jobs[id] = job
        return job
    }
    func jobResume(id: UInt64) throws -> FfiJobSnapshot { jobs[id]! }
    func jobForget(id: UInt64) throws { jobs.removeValue(forKey: id) }
    func jobResolve(id: UInt64, choice: FfiConflictChoice, applyToRemaining: Bool) throws -> FfiJobSnapshot {
        resolved.append((id, choice, applyToRemaining))
        var job = jobs[id]!
        job = FfiJobSnapshot(id: job.id, peer: job.peer, label: job.label, direction: job.direction, sender: job.sender, state: .transferring, totalBytes: job.totalBytes, doneBytes: job.doneBytes, filesTotal: job.filesTotal, filesDone: job.filesDone, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: nil, error: nil)
        jobs[id] = job
        return job
    }
}

private func entry(_ name: String, dir: Bool = false) -> FfiFsEntry {
    FfiFsEntry(name: name, kind: dir ? .dir : .file, size: dir ? 0 : 7, mtimeMs: 0, token: FfiEntryToken(dev: 3, ino: UInt64(abs(name.hashValue) + 1)))
}

private func page(_ names: [(String, Bool)]) -> FfiDirPage {
    let entries = names.map { entry($0.0, dir: $0.1) }
    return FfiDirPage(entries: entries, total: UInt64(entries.count), nextOffset: nil, truncated: false, nonUtf8Skipped: 0)
}

private func activeSession(id: UInt64 = 9, generation: UInt64 = 2) -> FfiSessionInfo {
    FfiSessionInfo(id: id, generation: generation, role: .controller, phase: .active, peerId: "peer", quality: nil, monitorWidth: 0, monitorHeight: 0)
}

@MainActor final class FilesModelTests: XCTestCase {
    private func model(_ backend: FakeFilesBackend, session: FfiSessionInfo? = activeSession()) -> FilesModel {
        FilesModel(backend: backend, session: { session }, peerId: "peer", localName: "This Mac", remoteName: "Linux", defaults: UserDefaults(suiteName: UUID().uuidString)!)
    }

    private func settle(_ seconds: TimeInterval = 2.0) {
        let end = Date().addingTimeInterval(seconds)
        while Date() < end { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
    }

    func testStaleLocalListingCannotPaintANavigatedPane() {
        let backend = FakeFilesBackend()
        backend.localPages[""] = page([("a.txt", false)])
        backend.localPages["sub"] = page([("b.txt", false)])
        backend.parkLocalPath = ""
        let model = self.model(backend)
        model.refreshLocal()
        // Wait until the first listing is parked inside the backend.
        XCTAssertEqual(backend.parked.wait(timeout: .now() + 5), .success)
        // Navigate away; the queued refresh mints a newer revision.
        model.navigateLocal(to: "sub")
        // Release the stale reply; the pane must keep showing the new folder.
        backend.release.signal()
        settle()
        XCTAssertEqual(model.local.page?.entries.map(\.name), ["b.txt"])
    }

    func testUploadSendsSelectionIntoTheRemoteFolder() {
        let backend = FakeFilesBackend()
        backend.localPages[""] = page([("a.txt", false), ("folder", true)])
        let model = self.model(backend)
        model.refreshLocal(); settle()
        model.local.selection = ["a.txt", "folder"]
        model.remoteRoot = 3
        model.remote.path = ["drop"]
        model.upload(); settle()
        XCTAssertEqual(backend.submitted.count, 2)
        XCTAssertTrue(backend.submitted.allSatisfy { $0.upload && $0.dstRoot == 3 && $0.dstRel == "drop" && $0.srcRoot == 0 })
        XCTAssertEqual(Set(backend.submitted.map(\.srcRel)), ["a.txt", "folder"])
        XCTAssertEqual(model.jobs.count, 2)
    }

    func testDownloadFetchesIntoTheLocalFolder() {
        let backend = FakeFilesBackend()
        backend.remotePages["stuff"] = page([("r.txt", false)])
        let model = self.model(backend)
        model.remote.path = ["stuff"]
        model.refreshRemote(); settle()
        model.remote.selection = ["r.txt"]
        model.download(); settle()
        XCTAssertEqual(backend.submitted.count, 1)
        let call = backend.submitted[0]
        XCTAssertFalse(call.upload)
        XCTAssertEqual(call.srcRel, "stuff/r.txt")
        XCTAssertEqual(call.dstRoot, 0)
        XCTAssertEqual(call.dstRel, "")
    }

    func testRenamePassesTheListingToken() {
        let backend = FakeFilesBackend()
        backend.localPages[""] = page([("old.txt", false)])
        backend.remotePages[""] = page([("rold.txt", false)])
        let model = self.model(backend)
        model.refreshLocal(); model.refreshRemote(); settle()
        let token = model.token(side: .local, name: "old.txt")
        XCTAssertEqual(token?.ino, model.local.page?.entries.first?.token.ino)
        model.renameLocal(name: "old.txt", newName: "new.txt", token: token)
        model.renameRemote(name: "rold.txt", newName: "rnew.txt", token: model.token(side: .remote, name: "rold.txt"))
        settle()
        XCTAssertEqual(backend.renamed.count, 2)
        XCTAssertFalse(backend.renamed[0].remote)
        XCTAssertTrue(backend.renamed[1].remote)
        XCTAssertEqual(backend.renamed[1].newName, "rnew.txt")
    }

    func testConflictResolveFlowRecordsChoiceAndScope() {
        let backend = FakeFilesBackend()
        let waiting = FfiJobSnapshot(id: 4, peer: "peer", label: "a.txt", direction: .upload, sender: true, state: .waitingDecision, totalBytes: 10, doneBytes: 0, filesTotal: 1, filesDone: 0, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: "a.txt", error: nil)
        backend.jobs[4] = waiting
        let model = self.model(backend)
        model.refreshJobs(); settle()
        XCTAssertEqual(model.jobs.first?.state, .waitingDecision)
        model.resolveJob(id: 4, choice: .keepBoth, applyToRemaining: true); settle()
        XCTAssertEqual(backend.resolved.count, 1)
        XCTAssertEqual(backend.resolved[0].choice, .keepBoth)
        XCTAssertTrue(backend.resolved[0].apply)
        XCTAssertEqual(model.jobs.first?.state, .transferring)
    }

    func testRemotePaneShowsDisconnectedWithoutSession() {
        let backend = FakeFilesBackend()
        let model = FilesModel(backend: backend, session: { nil }, peerId: "peer", localName: "This Mac", remoteName: "Linux", defaults: UserDefaults(suiteName: UUID().uuidString)!)
        model.refreshRemote()
        XCTAssertEqual(model.remote.message, "Not connected.")
        XCTAssertNil(model.remote.page)
    }

    func testFailedListingSurfacesAMessage() {
        let backend = FakeFilesBackend()
        backend.failList = true
        let model = self.model(backend)
        model.refreshLocal(); settle()
        XCTAssertNotNil(model.local.message)
    }

    func testSortingKeepsFoldersFirstAndOrdersFiles() {
        let entries = [
            FfiFsEntry(name: "file2", kind: .file, size: 100, mtimeMs: 3, token: entry("file2").token),
            FfiFsEntry(name: "file10", kind: .file, size: 2, mtimeMs: 8, token: entry("file10").token),
            entry("folder", dir: true)
        ]
        XCTAssertEqual(FilesSort.name.entries(entries).map(\.name), ["folder", "file2", "file10"])
        XCTAssertEqual(FilesSort.nameDescending.entries(entries).map(\.name), ["folder", "file10", "file2"])
        XCTAssertEqual(FilesSort.size.entries(entries).map(\.name), ["folder", "file2", "file10"])
        XCTAssertEqual(FilesSort.modified.entries(entries).map(\.name), ["folder", "file10", "file2"])
    }

    func testDropCapturesSourceAndTargetsFolderInBothDirections() {
        let backend = FakeFilesBackend()
        let model = model(backend)
        model.local.path = ["original"]
        model.local.selection = ["a.txt", "b.txt"]
        model.remote.path = ["destination"]
        model.remote.page = page([("sub", true)])
        let data = model.dragData(side: .local, name: "a.txt")!
        model.local.path = ["navigated"]
        XCTAssertFalse(model.dropData(data, onto: .local, folder: nil))
        XCTAssertTrue(model.dropData(data, onto: .remote, folder: "sub"))
        settle()
        XCTAssertEqual(backend.submitted.map(\.srcRel), ["original/a.txt", "original/b.txt"])
        XCTAssertEqual(backend.submitted.map(\.dstRel), ["destination/sub", "destination/sub"])
        XCTAssertTrue(backend.submitted.allSatisfy(\.upload))
        model.remote.selection = ["remote.txt"]
        let remoteData = model.dragData(side: .remote, name: "remote.txt")!
        XCTAssertTrue(model.dropData(remoteData, onto: .local, folder: nil))
        settle()
        XCTAssertEqual(backend.submitted.last?.srcRel, "destination/remote.txt")
        XCTAssertEqual(backend.submitted.last?.dstRel, "navigated")
        XCTAssertEqual(backend.submitted.last?.upload, false)
        let otherWindow = self.model(backend)
        XCTAssertFalse(otherWindow.dropData(data, onto: .remote, folder: nil))
    }

    func testSuccessfulLocationsRestoreForPeerWithoutLeakingToOtherPeer() {
        let backend = FakeFilesBackend()
        let suite = UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let model = FilesModel(backend: backend, session: { activeSession() }, peerId: "peer", localName: "Mac", remoteName: "Linux", defaults: defaults)
        model.local.root = 4; model.local.path = ["docs"]
        model.remoteRoot = 8; model.remote.path = ["downloads"]
        model.refreshLocal(); model.refreshRemote(); settle()
        let reopened = FilesModel(backend: backend, session: { activeSession() }, peerId: "peer", localName: "Mac", remoteName: "Linux", defaults: defaults)
        XCTAssertEqual(reopened.local.root, 4); XCTAssertEqual(reopened.local.path, ["docs"])
        XCTAssertEqual(reopened.remoteRoot, 8); XCTAssertEqual(reopened.remote.path, ["downloads"])
        let other = FilesModel(backend: backend, session: { activeSession() }, peerId: "other", localName: "Mac", remoteName: "Other", defaults: defaults)
        XCTAssertTrue(other.local.path.isEmpty); XCTAssertTrue(other.remote.path.isEmpty)
    }

    func testStateLabelsCoverEveryState() {
        XCTAssertEqual(FilesModel.stateLabel(.preparing), "Preparing")
        XCTAssertEqual(FilesModel.stateLabel(.waitingDecision), "Needs a decision")
        XCTAssertEqual(FilesModel.stateLabel(.complete), "Complete")
    }

    func testListAllFetchesBeyondTheFirstPage() {
        let backend = FakeFilesBackend()
        backend.fullLocalNames = (0..<250).map { "file-\($0).txt" }
        let model = self.model(backend)
        model.refreshLocal(); settle()
        // All 250 entries are visible, not just the first 200.
        XCTAssertEqual(model.local.page?.entries.count, 250)
        XCTAssertEqual(model.local.page?.total, 250)
    }

    func testCancelReachesBackendDuringBlockedPoll() {
        let backend = FakeFilesBackend()
        backend.jobs[7] = FfiJobSnapshot(id: 7, peer: "peer", label: "big", direction: .upload, sender: false, state: .transferring, totalBytes: 10, doneBytes: 0, filesTotal: 1, filesDone: 0, resumedFiles: 0, skippedFiles: 0, skippedSymlinks: 0, waitingFile: nil, error: nil)
        backend.parkPoll = true
        let model = self.model(backend)
        model.refreshJobs(); settle()
        XCTAssertEqual(model.jobs.count, 1)
        // Start a blocking poll on the transfer queue.
        model.pollJobs()
        XCTAssertEqual(backend.pollParked.wait(timeout: .now() + 5), .success)
        // Cancellation on the interactive queue must not wait behind it.
        model.cancelJob(id: 7)
        settle(1.0)
        XCTAssertTrue(backend.cancelSeen)
        backend.pollRelease.signal()
        settle()
        XCTAssertEqual(model.jobs.first?.state, .cancelled)
    }
    func testDesktopSelectionDuringStartupStagesItemWithoutUploading() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let url = root.appendingPathComponent("drop.txt")
        try Data("selected".utf8).write(to: url)
        let backend = FakeFilesBackend()
        backend.roots = [FfiFileRoot(id: 0, path: root.path)]
        backend.localPages[""] = page([("drop.txt", false)])
        let model = model(backend)
        model.start()
        model.selectUpload(url)
        settle()
        XCTAssertEqual(model.local.selection, ["drop.txt"])
        XCTAssertEqual(model.local.path, [])
        XCTAssertTrue(backend.submitted.isEmpty)
        XCTAssertEqual(model.notice, "Choose the remote destination, then Send.")
        model.upload(); settle()
        XCTAssertEqual(backend.submitted.map(\.srcRel), ["drop.txt"])
        model.stop()
    }

}
