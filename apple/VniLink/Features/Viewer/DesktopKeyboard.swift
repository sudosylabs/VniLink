import AppKit

struct DesktopKey: Equatable {
    enum Route { case local, physical }
    let route: Route
    let usage: UInt16
    let down: Bool
    let text: String
}

/// One source of held-key state for AppKit events and focus-loss releases.
struct DesktopKeyboard {
    private(set) var held = Set<UInt16>()
    mutating func key(code: UInt16, down: Bool, text: String = "") -> DesktopKey? {
        guard let usage = MacKeys.usage[code] else { return nil }
        if down { held.insert(usage) }
        else if held.remove(usage) == nil { return nil }
        // AppKit represents arrows/F-keys as private-use characters, not typed
        // text. Control characters (including Ctrl-Space's NUL) are also keys.
        let typed = text.unicodeScalars.allSatisfy {
            $0.properties.generalCategory != .control && !(0xf700...0xf8ff).contains($0.value)
        }
        return DesktopKey(route: .local, usage: usage, down: down, text: down && typed ? text : "")
    }
    mutating func modifier(code: UInt16, flags: NSEvent.ModifierFlags) -> DesktopKey? {
        guard var usage = MacKeys.usage[code], let mask = MacKeys.modifiers[code] else { return nil }
        // A focus-time snapshot cannot identify the physical side. If it
        // seeded the left key, a right-side release must release that alias.
        if !flags.contains(mask), !held.contains(usage),
           let group = MacKeys.orderedModifierGroups.first(where: { $0.0 == mask })?.1,
           let alias = group.first(where: { held.contains($0) }) {
            usage = alias
        }
        let down = flags.contains(mask) && (code == 57 || !held.contains(usage))
        if down { held.insert(usage) } else { held.remove(usage) }
        return DesktopKey(route: .local, usage: usage, down: down, text: "")
    }
    mutating func synchronize(_ flags: NSEvent.ModifierFlags) -> [DesktopKey] {
        var result: [DesktopKey] = []
        for (mask, group) in MacKeys.orderedModifierGroups {
            let pressed = group.filter { held.contains($0) }
            if flags.contains(mask), pressed.isEmpty {
                held.insert(group[0])
                result.append(DesktopKey(route: .local, usage: group[0], down: true, text: ""))
            } else if !flags.contains(mask) {
                for usage in pressed {
                    held.remove(usage)
                    result.append(DesktopKey(route: .local, usage: usage, down: false, text: ""))
                }
            }
        }
        return result
    }
    mutating func release() -> [DesktopKey] {
        defer { held.removeAll() }
        return held.sorted().map { DesktopKey(route: .local, usage: $0, down: false, text: "") }
    }
}

// macOS hardware virtual-key codes to USB HID keyboard usages.
enum MacKeys {
    static let orderedModifierGroups: [(NSEvent.ModifierFlags, [UInt16])] = [
        (.command, [227, 231]), (.control, [224, 228]), (.shift, [225, 229]), (.option, [226, 230])
    ]
    static let usage: [UInt16: UInt16] = [
        0:4,1:22,2:7,3:9,4:11,5:10,6:29,7:27,8:6,9:25,11:5,
        12:20,13:26,14:8,15:21,16:28,17:23,18:30,19:31,20:32,21:33,
        22:35,23:34,24:46,25:38,26:36,27:45,28:37,29:39,30:48,31:18,
        32:24,33:47,34:12,35:19,36:40,37:15,38:13,39:52,40:14,41:51,
        42:49,43:54,44:56,45:17,46:16,47:55,48:43,49:44,50:53,51:42,
        53:41,54:231,55:227,56:225,57:57,58:226,59:224,60:229,61:230,62:228,
        65:99,67:85,69:87,71:83,75:84,76:88,78:86,81:103,
        82:98,83:89,84:90,85:91,86:92,87:93,88:94,89:95,91:96,92:97,
        96:62,97:63,98:64,99:60,100:65,101:66,103:68,105:70,107:71,
        109:67,111:69,114:73,115:74,116:75,117:76,118:61,119:77,120:59,
        121:78,122:58,123:80,124:79,125:81,126:82
    ]
    static let modifiers: [UInt16: NSEvent.ModifierFlags] = [54:.command,55:.command,56:.shift,60:.shift,58:.option,61:.option,59:.control,62:.control,57:.capsLock]
}
