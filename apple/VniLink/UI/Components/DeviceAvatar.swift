import SwiftUI

/// Adapted from VniDrop's saved-device avatar, with VniLink's desktop symbol.
struct DeviceAvatar: View {
    var size: CGFloat = 40
    var body: some View {
        Image(systemName: "display")
            .font(.system(size: size * 0.45))
            .foregroundStyle(.secondary)
            .frame(width: size, height: size)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: size * 0.225))
    }
}
