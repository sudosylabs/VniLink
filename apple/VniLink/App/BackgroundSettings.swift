import AppKit
import Combine
import ServiceManagement

/// Lifecycle preferences contain no installation secrets. Login registration is
/// read from macOS rather than a cached preference that can drift from Settings.
@MainActor final class BackgroundSettings: ObservableObject {
    static let shared = BackgroundSettings()
    @Published var keepRunning = UserDefaults.standard.object(forKey: "keepRunning") as? Bool ?? true {
        didSet { UserDefaults.standard.set(keepRunning, forKey: "keepRunning") }
    }
    @Published var resumeSharing = UserDefaults.standard.bool(forKey: "resumeSharing") {
        didSet { UserDefaults.standard.set(resumeSharing, forKey: "resumeSharing") }
    }
    @Published var resumeInput = UserDefaults.standard.bool(forKey: "resumeInput") {
        didSet { UserDefaults.standard.set(resumeInput, forKey: "resumeInput") }
    }
    @Published private(set) var loginStatus = SMAppService.mainApp.status
    var launchesAtLogin: Bool { loginStatus == .enabled || loginStatus == .requiresApproval }
    func refresh() { loginStatus = SMAppService.mainApp.status }
    func setLogin(_ enabled: Bool) {
        do {
            if enabled { try SMAppService.mainApp.register() }
            else { try SMAppService.mainApp.unregister() }
        } catch { AppModel.shared.error = "Could not change launch at login: \(error.localizedDescription)" }
        refresh()
    }
}
