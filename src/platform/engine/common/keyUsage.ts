/**
 * USB HID keyboard usages (page 0x07) by `KeyboardEvent.code`: the physical
 * key, as the engine's key input names it, whatever the layout prints on it.
 * The engine's keyboard mode decides what the other computer types.
 */
const usages: Readonly<Record<string, number>> = (() => {
	const table: Record<string, number> = {};
	for (let index = 0; index < 26; index++) {
		table[`Key${String.fromCharCode(65 + index)}`] = 0x04 + index;
	}
	for (let digit = 1; digit <= 9; digit++) {
		table[`Digit${String(digit)}`] = 0x1d + digit;
		table[`Numpad${String(digit)}`] = 0x58 + digit;
	}
	table.Digit0 = 0x27;
	table.Numpad0 = 0x62;
	for (let key = 1; key <= 12; key++) {
		table[`F${String(key)}`] = 0x39 + key;
	}
	for (let key = 13; key <= 24; key++) {
		table[`F${String(key)}`] = 0x5b + key;
	}
	return Object.assign(table, {
		Enter: 0x28, Escape: 0x29, Backspace: 0x2a, Tab: 0x2b, Space: 0x2c,
		Minus: 0x2d, Equal: 0x2e, BracketLeft: 0x2f, BracketRight: 0x30, Backslash: 0x31,
		Semicolon: 0x33, Quote: 0x34, Backquote: 0x35, Comma: 0x36, Period: 0x37, Slash: 0x38,
		CapsLock: 0x39, PrintScreen: 0x46, ScrollLock: 0x47, Pause: 0x48,
		Insert: 0x49, Home: 0x4a, PageUp: 0x4b, Delete: 0x4c, End: 0x4d, PageDown: 0x4e,
		ArrowRight: 0x4f, ArrowLeft: 0x50, ArrowDown: 0x51, ArrowUp: 0x52,
		NumLock: 0x53, NumpadDivide: 0x54, NumpadMultiply: 0x55, NumpadSubtract: 0x56, NumpadAdd: 0x57,
		NumpadEnter: 0x58, NumpadDecimal: 0x63, IntlBackslash: 0x64, ContextMenu: 0x65, NumpadEqual: 0x67,
		AudioVolumeMute: 0x7f, AudioVolumeUp: 0x80, AudioVolumeDown: 0x81,
		IntlRo: 0x87, IntlYen: 0x89, Lang1: 0x90, Lang2: 0x91,
		ControlLeft: 0xe0, ShiftLeft: 0xe1, AltLeft: 0xe2, MetaLeft: 0xe3,
		ControlRight: 0xe4, ShiftRight: 0xe5, AltRight: 0xe6, MetaRight: 0xe7,
	});
})();

/** The HID usage of the key `code` names, or `undefined` for a key the engine has no usage for. */
export function hidUsage(code: string): number | undefined {
	return Object.hasOwn(usages, code) ? usages[code] : undefined;
}
