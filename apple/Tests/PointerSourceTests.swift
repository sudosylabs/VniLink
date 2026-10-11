import CoreGraphics
import XCTest
import VnilinkCore
@testable import DesktopInteraction

final class PointerSourceTests: XCTestCase {
    private func display(
        id: UInt64, x: Int32 = 0, y: Int32 = 0,
        width: UInt16 = 1920, height: UInt16 = 1080, revision: UInt64 = 3
    ) -> FfiDisplay {
        FfiDisplay(
            id: id, originX: x, originY: y, width: width, height: height,
            rotationDeg: 0, scalePercent: 100, revision: revision)
    }

    func testSingleMatchingDisplayIsSelected() {
        let source = PointerSource.select(
            displays: [display(id: 7)], frameDisplayId: 0, frameRevision: 0,
            frameWidth: 1920, frameHeight: 1080)
        XCTAssertEqual(
            source,
            PointerSource(
                id: 7, revision: 3, originX: 0, originY: 0, width: 1920, height: 1080))
    }

    func testMatchingDisplayWinsOverFirstListItem() {
        let displays = [
            display(id: 1, width: 1280, height: 720),
            display(id: 2, x: 1280, width: 1920, height: 1080),
        ]
        let source = PointerSource.select(
            displays: displays, frameDisplayId: 0, frameRevision: 0,
            frameWidth: 1920, frameHeight: 1080)
        XCTAssertEqual(source?.id, 2)
        let mapped = source?.map(x: 10, y: 20)
        XCTAssertEqual(mapped?.0, 1290)
        XCTAssertEqual(mapped?.1, 20)
    }

    func testTwinDisplaysFenceThePointer() {
        let displays = [
            display(id: 1, width: 1920, height: 1080),
            display(id: 2, x: 1920, width: 1920, height: 1080),
        ]
        XCTAssertNil(PointerSource.select(
            displays: displays, frameDisplayId: 0, frameRevision: 0,
            frameWidth: 1920, frameHeight: 1080))
    }

    func testStampedFrameBindsItsTwinSource() {
        let displays = [
            display(id: 1, width: 1920, height: 1080),
            display(id: 2, x: 1920, width: 1920, height: 1080),
        ]
        // Same size, distinct ids: the stamp disambiguates what geometry
        // cannot, and order never wins over the stamp.
        let right = PointerSource.select(
            displays: displays, frameDisplayId: 2, frameRevision: 3,
            frameWidth: 1920, frameHeight: 1080)
        XCTAssertEqual(right?.id, 2)
        XCTAssertEqual(right?.map(x: 10, y: 20)?.0, 1930)
        let left = PointerSource.select(
            displays: [displays[1], displays[0]], frameDisplayId: 1, frameRevision: 3,
            frameWidth: 1920, frameHeight: 1080)
        XCTAssertEqual(left?.id, 1)
        XCTAssertEqual(left?.map(x: 10, y: 20)?.0, 10)
        // The stamp wins over geometry: a resized picture still names its
        // source, and mapping uses the live topology entry.
        let resized = PointerSource.select(
            displays: displays, frameDisplayId: 2, frameRevision: 3,
            frameWidth: 800, frameHeight: 600)
        XCTAssertEqual(resized?.id, 2)
    }

    func testMissingOrUninitializedSourceFencesThePointer() {
        XCTAssertNil(PointerSource.select(
            displays: [display(id: 1, width: 1280, height: 720)],
            frameDisplayId: 0, frameRevision: 0, frameWidth: 1920, frameHeight: 1080))
        XCTAssertNil(PointerSource.select(
            displays: [], frameDisplayId: 0, frameRevision: 0, frameWidth: 1920, frameHeight: 1080))
        XCTAssertNil(PointerSource.select(
            displays: [display(id: 1, revision: 0)],
            frameDisplayId: 0, frameRevision: 0, frameWidth: 1920, frameHeight: 1080))
        // A stamped but unknown or uninitialized source fences rather than
        // falling back to geometry.
        XCTAssertNil(PointerSource.select(
            displays: [display(id: 1)],
            frameDisplayId: 9, frameRevision: 3, frameWidth: 1920, frameHeight: 1080))
        XCTAssertNil(PointerSource.select(
            displays: [display(id: 1, revision: 0)],
            frameDisplayId: 1, frameRevision: 3, frameWidth: 1920, frameHeight: 1080))
    }

    func testRevisionMismatchFencesTheStaleHeldPicture() {
        let live = display(id: 1, revision: 9)
        // Same id, older pixels: fenced, never relabeled current.
        XCTAssertNil(PointerSource.select(
            displays: [live], frameDisplayId: 1, frameRevision: 3,
            frameWidth: 1920, frameHeight: 1080))
        // Same id, newer pixels than the known topology: fenced.
        XCTAssertNil(PointerSource.select(
            displays: [display(id: 1, revision: 3)],
            frameDisplayId: 1, frameRevision: 9,
            frameWidth: 1920, frameHeight: 1080))
        // Exact match maps to the live entry.
        let source = PointerSource.select(
            displays: [live], frameDisplayId: 1, frameRevision: 9,
            frameWidth: 1920, frameHeight: 1080)
        XCTAssertEqual(source?.revision, 9)
        // Unknown frame revisions never map, even for a live id.
        XCTAssertNil(PointerSource.select(
            displays: [live], frameDisplayId: 1, frameRevision: 0,
            frameWidth: 1920, frameHeight: 1080))
    }

    func testMappingAddsTheSourceOrigin() {
        let source = PointerSource(
            id: 2, revision: 5, originX: -1920, originY: 100, width: 1920, height: 1080)
        let origin = source.map(x: 0, y: 0)
        XCTAssertEqual(origin?.0, -1920)
        XCTAssertEqual(origin?.1, 100)
        let corner = source.map(x: 1919, y: 1079)
        XCTAssertEqual(corner?.0, -1)
        XCTAssertEqual(corner?.1, 1179)
    }

    func testMapFencesPointsPastTheSourceExtent() {
        let source = PointerSource(
            id: 2, revision: 5, originX: 1920, originY: 0, width: 1280, height: 720)
        XCTAssertNotNil(source.map(x: 1279, y: 719))
        XCTAssertNil(source.map(x: 1280, y: 0))
        XCTAssertNil(source.map(x: 0, y: 720))
        XCTAssertNil(source.map(x: 1919, y: 1079))
    }

    func testFractionRejectsLetterboxBars() {
        let rect = CGRect(x: 0, y: 219, width: 1000, height: 562)
        let middle = PointerSource.fraction(
            point: CGPoint(x: 500, y: 500), in: rect)
        XCTAssertEqual(middle?.0 ?? -1, 0.5, accuracy: 0.001)
        XCTAssertEqual(middle?.1 ?? -1, 0.5, accuracy: 0.001)
        XCTAssertNil(PointerSource.fraction(
            point: CGPoint(x: 500, y: 100), in: rect))
        XCTAssertNil(PointerSource.fraction(
            point: CGPoint(x: 500, y: 950), in: rect))
        XCTAssertNil(PointerSource.fraction(
            point: CGPoint(x: 500, y: 500), in: .zero))
    }

    /// SPEC17: initial 1920x1080, secondary 1280x720 at x=1920. A
    /// center click on the secondary picture lands at desktop
    /// (2560, 360) — the old handshake scale sent (2880, 540) and
    /// fenced the right third as out of bounds.
    func testSecondarySourceMapsWithItsOwnExtent() {
        let secondary = PointerSource(
            id: 2, revision: 3, originX: 1920, originY: 0, width: 1280, height: 720)
        let rect = CGRect(x: 0, y: 0, width: 1280, height: 720)
        let map = { (x: CGFloat, y: CGFloat) -> (Int32, Int32)? in
            guard let (fx, fy) = PointerSource.fraction(
                point: CGPoint(x: x, y: y), in: rect)
            else { return nil }
            let (px, py) = secondary.normalize(fractionX: fx, fractionY: fy)
            return secondary.map(x: px, y: py)
        }
        XCTAssertEqual(map(640, 360)?.0, 2560)
        XCTAssertEqual(map(640, 360)?.1, 360)
        XCTAssertEqual(map(0, 0)?.0, 1920)
        XCTAssertEqual(map(1200, 360)?.0, 3120)
        XCTAssertEqual(map(1279, 719)?.0, 3199)
        XCTAssertNil(map(1280, 360))
    }

    /// SPEC17: a quality-scaled decode still addresses source pixels,
    /// and negative origins compose without shifting the point.
    func testScaledFrameAndNegativeOriginKeepTheDesktopPoint() {
        let secondary = PointerSource(
            id: 2, revision: 3, originX: 1920, originY: 0, width: 1280, height: 720)
        let small = CGRect(x: 0, y: 0, width: 640, height: 360)
        let (fx, fy) = PointerSource.fraction(
            point: CGPoint(x: 320, y: 180), in: small)!
        let (px, py) = secondary.normalize(fractionX: fx, fractionY: fy)
        XCTAssertEqual(px, 640)
        XCTAssertEqual(py, 360)
        XCTAssertEqual(secondary.map(x: px, y: py)?.0, 2560)
        let left = PointerSource(
            id: 1, revision: 3, originX: -1280, originY: 0, width: 1280, height: 720)
        let (lx, ly) = left.normalize(fractionX: 0.5, fractionY: 0.5)
        XCTAssertEqual(left.map(x: lx, y: ly)?.0, -640)
        XCTAssertEqual(left.map(x: lx, y: ly)?.1, 360)
    }
}
