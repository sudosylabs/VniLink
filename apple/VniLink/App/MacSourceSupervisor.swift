import CoreGraphics
import Foundation
import VnilinkCore

/// Opens one native capture for a display. Production opens through
/// `VnilinkCore.macHost`; tests inject fakes.
protocol MacDisplayProducer {
    func open(
        _ display: MacSourceSupervisor.Display, cursorSupported: Bool
    ) throws -> MacHostProtocol
}

/// Production producer: one `macHost` call per display. Concurrent hosts
/// share the session and topology through the FFI host registry, and the
/// last stop tears shared state down.
final class CoreMacDisplayProducer: MacDisplayProducer {
    private let core: VnilinkCore
    init(core: VnilinkCore) { self.core = core }
    func open(
        _ display: MacSourceSupervisor.Display, cursorSupported: Bool
    ) throws -> MacHostProtocol {
        try core.macHost(
            displayId: display.id, width: display.width, height: display.height,
            pixelWidth: display.pixelWidth, pixelHeight: display.pixelHeight,
            cursorSupported: cursorSupported)
    }
}

/// Nonvisual supervisor over per-display native captures.
///
/// The supervisor observes requested source ids (remote Desktop selections)
/// and the live display set, then holds exactly the wanted owners: every
/// requested id that names a live display, plus the first live display
/// (which serves unselected viewers). Opening, closing or failing one
/// display never touches its siblings; a failed open is retried on the
/// next pass, and leaving the wanted set stops just that owner.
///
/// Confine to one serial queue (the app uses its host queue): the
/// supervisor is not internally synchronized.
final class MacSourceSupervisor {
    /// One supported display: its OS id and capture geometry.
    struct Display: Hashable {
        let id: UInt32
        let width: UInt16
        let height: UInt16
        let pixelWidth: UInt16
        let pixelHeight: UInt16
    }

    private let requestedIds: () -> [UInt64]
    private let liveDisplays: () -> [Display]
    private let producer: MacDisplayProducer
    private let cursorSupported: Bool
    /// Immutable identity fences snapshots delivered across queues.
    let generation = UUID()
    struct FailureSnapshot {
        let generation: UUID
        let message: String?
        func message(ifCurrent current: UUID?) -> String? {
            generation == current ? message : nil
        }
    }
    private var stopped = false
    private let sourceFailed: (UInt32, String) -> Void
    private var geometries: [UInt32: Display] = [:]
    private var hosts: [UInt32: MacHostProtocol] = [:]
    private(set) var lastError: String?

    init(
        requestedIds: @escaping () -> [UInt64],
        liveDisplays: @escaping () -> [Display],
        producer: MacDisplayProducer,
        cursorSupported: Bool,
        sourceFailed: @escaping (UInt32, String) -> Void = { _, _ in }
    ) {
        self.requestedIds = requestedIds
        self.liveDisplays = liveDisplays
        self.producer = producer
        self.cursorSupported = cursorSupported
        self.sourceFailed = sourceFailed
    }

    /// Live capture ids, ascending.
    var currentIds: [UInt32] { hosts.keys.sorted() }

    /// First failure across the last reconcile and every live host,
    /// ascending by display id.
    var firstError: String? {
        if let lastError { return lastError }
        for id in currentIds {
            if let error = hosts[id]?.error() { return error }
        }
        return nil
    }

    /// Exact wanted set: every requested id that names a live display,
    /// plus the first live display. Stale selections name nothing and
    /// hold no owner; id 0 and unknown ids never open.
    static func wantedIds(requested: [UInt64], live: [Display]) -> Set<UInt32> {
        let liveIds = Set(live.map(\.id).filter { $0 != 0 })
        var wanted = Set(
            requested.compactMap { UInt32(exactly: $0) }.filter { liveIds.contains($0) })
        if let first = live.first(where: { $0.id != 0 }) { wanted.insert(first.id) }
        return wanted
    }

    /// Opens missing wanted captures and retires stale ones, both in
    /// ascending id order for determinism.
    func reconcile() {
        guard !stopped else { return }
        lastError = nil
        retireFailedHosts()
        let live = liveDisplays()
        let byId = Dictionary(uniqueKeysWithValues: live.map { ($0.id, $0) })
        let wanted = Self.wantedIds(requested: requestedIds(), live: live)
        for id in hosts.keys.filter({ !wanted.contains($0) || geometries[$0] != byId[$0] }).sorted() {
            hosts[id]?.stop()
            hosts[id] = nil
            geometries[id] = nil
        }
        for id in wanted.sorted() {
            guard hosts[id] == nil, let display = byId[id] else { continue }
            do {
                hosts[id] = try producer.open(display, cursorSupported: cursorSupported)
                geometries[id] = display
            } catch {
                reportFailure(id, String(describing: error))
            }
        }
    }

    private func reportFailure(_ id: UInt32, _ message: String) {
        let qualified = "Display \(id): \(message)"
        lastError = qualified
        sourceFailed(id, qualified)
    }

    private func retireFailedHosts() {
        let live = Dictionary(uniqueKeysWithValues: liveDisplays().map { ($0.id, $0) })
        for id in currentIds {
            guard let message = hosts[id]?.error() else { continue }
            let sameGeometry = geometries[id] == live[id]
            hosts[id]?.stop()
            hosts[id] = nil
            geometries[id] = nil
            // A retired geometry's worker must not fail a replacement source.
            if sameGeometry { reportFailure(id, message) }
        }
    }

    /// Production refresh consumer: settle runtime failures on the host queue
    /// and return only an immutable, generation-qualified report to the UI.
    func refreshSnapshot() -> FailureSnapshot {
        if !stopped { retireFailedHosts() }
        return FailureSnapshot(generation: generation, message: stopped ? nil : lastError)
    }

    /// Runs `body` over every live host, ascending by display id.
    func forEachHost(_ body: (MacHostProtocol) -> Void) {
        for id in currentIds {
            if let host = hosts[id] { body(host) }
        }
    }

    /// Stops every capture. Siblings stop independently; nothing streams
    /// after this returns.
    func stopAll() {
        stopped = true
        for id in currentIds { hosts[id]?.stop() }
        hosts.removeAll()
        geometries.removeAll()
        lastError = nil
    }
}
