import XCTest
import VnilinkCore
@testable import MacSources

/// Fake capture: records stops and vends display-tagged frame stamps so
/// the test proves each producer serves its own source.
final class FakeMacHost: MacHostProtocol {
    let displayId: UInt32
    private(set) var stops = 0
    private var stamp: UInt64 = 0
    var errorMessage: String?
    init(displayId: UInt32) { self.displayId = displayId }
    func error() -> String? { errorMessage }
    func stop() { stops += 1 }
    func updateCursor(update: MacCursorUpdate) throws {}
    func nextFrame() -> (UInt32, UInt64) {
        stamp += 1
        return (displayId, stamp)
    }
}

final class FakeMacProducer: MacDisplayProducer {
    private(set) var opened: [UInt32] = []
    private(set) var hosts: [UInt32: FakeMacHost] = [:]
    var failuresLeft = 0
    func open(
        _ display: MacSourceSupervisor.Display, cursorSupported: Bool
    ) throws -> MacHostProtocol {
        opened.append(display.id)
        if failuresLeft > 0 {
            failuresLeft -= 1
            throw FfiError.Unsupported(message: "display unavailable")
        }
        let host = FakeMacHost(displayId: display.id)
        hosts[display.id] = host
        return host
    }
}

/// Two injected producers behind one dispatcher: routing picks the
/// producer per display id.
final class RoutingMacProducer: MacDisplayProducer {
    let first: FakeMacProducer
    let second: FakeMacProducer
    let route: (UInt32) -> FakeMacProducer
    init(first: FakeMacProducer, second: FakeMacProducer, route: @escaping (UInt32) -> FakeMacProducer) {
        self.first = first
        self.second = second
        self.route = route
    }
    func open(
        _ display: MacSourceSupervisor.Display, cursorSupported: Bool
    ) throws -> MacHostProtocol {
        try route(display.id).open(display, cursorSupported: cursorSupported)
    }
}

final class MacSourceSupervisorTests: XCTestCase {
    private func display(
        _ id: UInt32, width: UInt16 = 1920, height: UInt16 = 1080
    ) -> MacSourceSupervisor.Display {
        MacSourceSupervisor.Display(
            id: id, width: width, height: height, pixelWidth: width, pixelHeight: height)
    }

    private func harness(
        live: [MacSourceSupervisor.Display], requested: [UInt64] = [],
        failures: (Int, Int) = (0, 0),
        sourceFailed: @escaping (UInt32, String) -> Void = { _, _ in }
    ) -> (MacSourceSupervisor, FakeMacProducer, FakeMacProducer,
        FakeBox<[MacSourceSupervisor.Display]>, FakeBox<[UInt64]>) {
        let liveBox = FakeBox(live)
        let requestedBox = FakeBox(requested)
        let first = FakeMacProducer()
        let second = FakeMacProducer()
        first.failuresLeft = failures.0
        second.failuresLeft = failures.1
        let router = RoutingMacProducer(first: first, second: second) { $0 == 1 ? first : second }
        let supervisor = MacSourceSupervisor(
            requestedIds: { requestedBox.value },
            liveDisplays: { liveBox.value },
            producer: router,
            cursorSupported: true, sourceFailed: sourceFailed)
        return (supervisor, first, second, liveBox, requestedBox)
    }

    func testOpensFirstLiveWhenNothingRequested() {
        let (supervisor, first, second, _, _) = harness(live: [display(1), display(2)])
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1])
        XCTAssertEqual(first.opened, [1])
        XCTAssertTrue(second.opened.isEmpty)
        XCTAssertNil(supervisor.lastError)
    }

    func testSecondaryRequestRoutesToSecondProducerWithDistinctStamps() {
        let (supervisor, first, second, _, _) = harness(
            live: [display(1), display(2)], requested: [2])
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1, 2])
        XCTAssertEqual(first.opened, [1])
        XCTAssertEqual(second.opened, [2])
        // A second pass opens nothing again: both owners keep streaming.
        supervisor.reconcile()
        XCTAssertEqual(first.opened, [1])
        XCTAssertEqual(second.opened, [2])
        // Distinct frame stamps prove independent producers serve their
        // own source with no cross-talk.
        let firstFrame = first.hosts[1]!.nextFrame()
        let secondFrame = second.hosts[2]!.nextFrame()
        XCTAssertEqual(firstFrame.0, 1)
        XCTAssertEqual(secondFrame.0, 2)
        XCTAssertNotEqual(firstFrame.1, 0)
        XCTAssertNotEqual(secondFrame.1, 0)
        XCTAssertEqual(first.hosts[1]!.nextFrame().1, 2)
        XCTAssertEqual(second.hosts[2]!.nextFrame().1, 2)
    }

    func testUnknownAndZeroRequestsOpenNothing() {
        let (supervisor, first, second, _, _) = harness(
            live: [display(1), display(2)], requested: [0, 99, 2, UInt64(UInt32.max) + 1])
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1, 2])
        XCTAssertEqual(first.opened, [1])
        XCTAssertEqual(second.opened, [2])
    }

    func testDeselectedSourceClosesIndependently() {
        let (supervisor, first, second, _, requested) = harness(
            live: [display(1), display(2)], requested: [2])
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1, 2])
        requested.value = []
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1])
        XCTAssertEqual(first.hosts[1]!.stops, 0)
        XCTAssertEqual(second.hosts[2]!.stops, 1)
        // The surviving owner keeps serving its own source.
        XCTAssertEqual(first.hosts[1]!.nextFrame().0, 1)
    }

    func testLiveRemovalRetiresOwner() {
        let (supervisor, first, second, live, _) = harness(
            live: [display(1), display(2)], requested: [2])
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1, 2])
        live.value = [display(1)]
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1])
        XCTAssertEqual(first.hosts[1]!.stops, 0)
        XCTAssertEqual(second.hosts[2]!.stops, 1)
    }

    func testOpenFailureRecordsErrorAndRetries() {
        let (supervisor, _, second, _, _) = harness(
            live: [display(1), display(2)], requested: [2], failures: (0, 1))
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1])
        XCTAssertNotNil(supervisor.lastError)
        XCTAssertEqual(supervisor.firstError, supervisor.lastError)
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1, 2])
        XCTAssertNil(supervisor.lastError)
        XCTAssertEqual(second.opened, [2, 2])
    }

    func testFirstErrorFallsThroughToLiveHosts() {
        let (supervisor, first, _, _, _) = harness(live: [display(1)])
        supervisor.reconcile()
        XCTAssertNil(supervisor.firstError)
        first.hosts[1]!.errorMessage = "worker lost the display"
        XCTAssertEqual(supervisor.firstError, "worker lost the display")
    }

    func testSameIdGeometryChangeReopensOnlyChangedSource() {
        let (supervisor, first, second, live, _) = harness(
            live: [display(1), display(2)], requested: [2])
        supervisor.reconcile()
        let retired = first.hosts[1]!
        let sibling = second.hosts[2]!
        live.value = [display(1, width: 1280, height: 720), display(2)]
        supervisor.reconcile()
        XCTAssertEqual(retired.stops, 1)
        XCTAssertEqual(first.opened, [1, 1])
        XCTAssertFalse(first.hosts[1] === retired)
        XCTAssertTrue(second.hosts[2] === sibling)
        XCTAssertEqual(sibling.stops, 0)
        XCTAssertEqual(supervisor.currentIds, [1, 2])
    }

    func testRefreshConsumerRetiresOnlyFailedSourceAndRetries() {
        var failed: [(UInt32, String)] = []
        let (supervisor, first, second, _, _) = harness(
            live: [display(1), display(2)], requested: [2],
            sourceFailed: { failed.append(($0, $1)) })
        supervisor.reconcile()
        let healthy = first.hosts[1]!
        let retired = second.hosts[2]!
        retired.errorMessage = "secondary worker failed"
        let report = supervisor.refreshSnapshot()
        XCTAssertEqual(report.message(ifCurrent: supervisor.generation), "Display 2: secondary worker failed")
        XCTAssertEqual(failed.map { $0.0 }, [2])
        XCTAssertEqual(supervisor.currentIds, [1])
        XCTAssertEqual(healthy.stops, 0)
        XCTAssertEqual(retired.stops, 1)
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1, 2])
        XCTAssertTrue(first.hosts[1] === healthy)
        XCTAssertFalse(second.hosts[2] === retired)
        XCTAssertNil(supervisor.refreshSnapshot().message)
    }

    func testRefreshConsumerPreservesHealthySourceAfterOpenFailure() {
        var failed: [UInt32] = []
        let (supervisor, first, _, _, _) = harness(
            live: [display(1), display(2)], requested: [2], failures: (0, 1),
            sourceFailed: { id, _ in failed.append(id) })
        supervisor.reconcile()
        XCTAssertEqual(failed, [2])
        XCTAssertEqual(supervisor.currentIds, [1])
        XCTAssertEqual(first.hosts[1]!.stops, 0)
        XCTAssertTrue(supervisor.refreshSnapshot().message!.hasPrefix("Display 2:"))
    }

    func testRetiredSupervisorCannotReopenOrReportIntoReplacement() {
        let (old, _, _, _, _) = harness(live: [display(1)], failures: (1, 0))
        old.reconcile()
        let queued = old.refreshSnapshot()
        let (replacement, _, _, _, _) = harness(live: [display(1)])
        old.stopAll()
        old.reconcile()
        XCTAssertTrue(old.currentIds.isEmpty)
        XCTAssertNil(old.refreshSnapshot().message)
        XCTAssertNil(queued.message(ifCurrent: replacement.generation))
        XCTAssertNil(queued.message(ifCurrent: nil))
    }

    func testRetiredGeometryErrorCannotFailReplacementSelection() {
        var failures: [UInt32] = []
        let (supervisor, first, _, live, _) = harness(live: [display(1)],
            sourceFailed: { id, _ in failures.append(id) })
        supervisor.reconcile()
        first.hosts[1]!.errorMessage = "old geometry failed"
        live.value = [display(1, width: 1280, height: 720)]
        XCTAssertNil(supervisor.refreshSnapshot().message)
        XCTAssertTrue(failures.isEmpty)
        supervisor.reconcile()
        XCTAssertEqual(supervisor.currentIds, [1])
    }

    func testStopAllClosesEverySource() {
        let (supervisor, first, second, _, _) = harness(
            live: [display(1), display(2)], requested: [2])
        supervisor.reconcile()
        var cursorUpdates = 0
        supervisor.forEachHost { _ in cursorUpdates += 1 }
        XCTAssertEqual(cursorUpdates, 2)
        supervisor.stopAll()
        XCTAssertTrue(supervisor.currentIds.isEmpty)
        XCTAssertEqual(first.hosts[1]!.stops, 1)
        XCTAssertEqual(second.hosts[2]!.stops, 1)
        XCTAssertNil(supervisor.lastError)
    }
}

/// Mutable box so provider closures observe test-side updates.
final class FakeBox<T> {
    var value: T
    init(_ value: T) { self.value = value }
}
