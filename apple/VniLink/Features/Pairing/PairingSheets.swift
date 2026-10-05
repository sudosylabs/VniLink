import SwiftUI
import VnilinkCore

struct PageHeading: View {
    let title: String
    let detail: String
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.title2.weight(.semibold))
            Text(detail).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }
}
struct AddComputerSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    @State private var token = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            PageHeading(title: "Add computer", detail: "Use a pairing file or invitation created on your other computer.")
            Button("Open pairing file…") { model.choosePairingFile() }
            Divider()
            SecureField("Invitation token", text: $token).textFieldStyle(.roundedBorder)
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Accept invitation") { model.acceptToken(token) }.disabled(token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty).keyboardShortcut(.defaultAction)
            }
        }.padding(28).frame(width: 460)
    }
}
struct CodeSheet: View {
    let title: String
    let detail: String
    let action: String
    let confirm: (String) -> Void
    @Environment(\.dismiss) var dismiss
    @State private var digits = ""
    var valid: Bool { digits.utf8.count == 3 && digits.utf8.allSatisfy { $0 >= 48 && $0 <= 57 } }
    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            PageHeading(title: title, detail: detail)
            TextField("000", text: $digits).font(.system(size: 30, design: .monospaced)).textFieldStyle(.roundedBorder)
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer(); Button(action) { confirm(digits) }.disabled(!valid).keyboardShortcut(.defaultAction)
            }
        }.padding(28).frame(width: 420)
    }
}
