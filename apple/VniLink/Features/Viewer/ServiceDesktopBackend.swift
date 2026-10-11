import CoreGraphics
import Foundation
import VnilinkCore

/// One Desktop owner, independent of its authenticated peer connection and Files.
/// Open runs off the main thread; Close fences admission before native startup.
final class ServiceDesktopBackend: @unchecked Sendable {
    private let core: VnilinkCore
    private let lock = NSLock()
    private let opening = NSLock()
    private var admission: FfiServiceInfo?
    private var owner: DesktopService?
    private var closed = false
    private var revision: UInt64 = 0

    init(core: VnilinkCore) { self.core = core }
    func open(_ session: FfiSessionInfo) throws -> DesktopService {
        opening.lock(); defer { opening.unlock() }
        lock.lock()
        guard !closed else { lock.unlock(); throw FfiError.NoSession }
        let previous = owner
        lock.unlock()
        if let previous, previous.info().generation == session.generation,
           previous.info().connectionId == session.id, !previous.isCancelled() { return previous }
        retire()
        // Retire changes the local epoch; Close may race while the native open waits.
        lock.lock(); let expected = revision; let ended = closed; lock.unlock()
        guard !ended else { throw FfiError.NoSession }
        let info = try core.attachControllerService(connectionId: session.id, generation: session.generation, kind: .desktop)
        lock.lock()
        guard !closed, revision == expected else {
            lock.unlock()
            try? core.closeControllerService(id: info.id, generation: info.generation)
            try? core.forgetControllerService(id: info.id)
            throw FfiError.NoSession
        }
        admission = info
        lock.unlock()
        let next: DesktopService
        do { next = try core.openDesktopService(id: info.id, generation: info.generation) }
        catch { retireIfCurrent(info); throw error }
        lock.lock()
        guard !closed, revision == expected else {
            lock.unlock(); next.close(); throw FfiError.NoSession
        }
        owner = next
        lock.unlock()
        return next
    }
    private func retireIfCurrent(_ info: FfiServiceInfo) {
        lock.lock()
        let matches = admission?.id == info.id && admission?.generation == info.generation
        lock.unlock()
        if matches { retire() }
    }
    func retire() {
        lock.lock()
        revision &+= 1
        let info = admission
        let previous = owner
        admission = nil; owner = nil
        lock.unlock()
        previous?.close()
        if let info {
            try? core.closeControllerService(id: info.id, generation: info.generation)
            try? core.forgetControllerService(id: info.id)
        }
    }
    func close() {
        lock.lock(); closed = true; lock.unlock()
        retire()
    }
    deinit { close() }
}

/// Pointer source bound to the displayed frame. A stamped frame binds its
/// carried (source id, revision) against the live topology entry, so twin
/// same-sized displays stay distinct and a stale held picture cannot be
/// relabeled with fresh geometry. Only unstamped whole-desktop pictures
/// fall back to unique-geometry matching; a zero, unknown, ambiguous or
/// revision-mismatched source fences the pointer until matching pixels
/// arrive.
struct PointerSource: Equatable {
    let id: UInt64
    let revision: UInt64
    let originX: Int32
    let originY: Int32
    let width: UInt16
    let height: UInt16
    static func select(
        displays: [FfiDisplay], frameDisplayId: UInt64, frameRevision: UInt64,
        frameWidth: UInt16, frameHeight: UInt16
    ) -> PointerSource? {
        if frameDisplayId != 0 {
            guard frameRevision != 0, let display = displays.first(where: {
                $0.id == frameDisplayId && $0.revision == frameRevision
            }) else { return nil }
            return PointerSource(
                id: display.id, revision: display.revision,
                originX: display.originX, originY: display.originY,
                width: display.width, height: display.height)
        }
        let candidates = displays.filter {
            $0.width == frameWidth && $0.height == frameHeight && $0.revision > 0
        }
        guard candidates.count == 1, let display = candidates.first else { return nil }
        return PointerSource(
            id: display.id, revision: display.revision,
            originX: display.originX, originY: display.originY,
            width: display.width, height: display.height)
    }
    /// Picture fraction of a point inside an aspect-fit rect. Points
    /// outside (letterbox bars) are not part of the remote desktop.
    static func fraction(point: CGPoint, in rect: CGRect) -> (CGFloat, CGFloat)? {
        guard rect.width > 0, rect.height > 0, rect.contains(point) else { return nil }
        return ((point.x - rect.minX) / rect.width, (point.y - rect.minY) / rect.height)
    }
    /// Scales a picture fraction onto this source's logical extent,
    /// clamping the rounded edge inside. The handshake monitor never
    /// participates: secondary sources keep their own size.
    func normalize(fractionX: CGFloat, fractionY: CGFloat) -> (UInt16, UInt16) {
        guard width > 0, height > 0 else { return (0, 0) }
        return (
            UInt16(min(CGFloat(width - 1), max(0, fractionX * CGFloat(width)))),
            UInt16(min(CGFloat(height - 1), max(0, fractionY * CGFloat(height))))
        )
    }
    /// Maps a source point onto desktop coordinates. Points past the
    /// source extent fence instead of landing on a neighbor display.
    func map(x: UInt16, y: UInt16) -> (Int32, Int32)? {
        guard x < width, y < height else { return nil }
        return (originX + Int32(x), originY + Int32(y))
    }
}
