/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { illegalArgument } from './errors.js';
import { KeyCode, ScanCode } from './keyCodes.js';
import { OperatingSystem } from './platform.js';

/**
 * Binary encoding strategy:
 * ```
 *    1111 11
 *    5432 1098 7654 3210
 *    ---- CSAW KKKK KKKK
 *  C = bit 11 = ctrlCmd flag
 *  S = bit 10 = shift flag
 *  A = bit 9 = alt flag
 *  W = bit 8 = winCtrl flag
 *  K = bits 0-7 = key code
 * ```
 */
const enum BinaryKeybindingsMask {
	CtrlCmd = (1 << 11) >>> 0,
	Shift = (1 << 10) >>> 0,
	Alt = (1 << 9) >>> 0,
	WinCtrl = (1 << 8) >>> 0,
	KeyCode = 0x000000FF
}

export function decodeKeybinding(keybinding: number | number[], OS: OperatingSystem): Keybinding | null {
	if (typeof keybinding === 'number') {
		if (keybinding === 0) {
			return null;
		}
		const firstChord = (keybinding & 0x0000FFFF) >>> 0;
		const secondChord = (keybinding & 0xFFFF0000) >>> 16;
		if (secondChord !== 0) {
			return new Keybinding([
				createSimpleKeybinding(firstChord, OS),
				createSimpleKeybinding(secondChord, OS)
			]);
		}
		return new Keybinding([createSimpleKeybinding(firstChord, OS)]);
	} else {
		const chords = [];
		for (let i = 0; i < keybinding.length; i++) {
			chords.push(createSimpleKeybinding(keybinding[i], OS));
		}
		return new Keybinding(chords);
	}
}

export function createSimpleKeybinding(keybinding: number, OS: OperatingSystem): KeyCodeChord {

	const ctrlCmd = (keybinding & BinaryKeybindingsMask.CtrlCmd ? true : false);
	const winCtrl = (keybinding & BinaryKeybindingsMask.WinCtrl ? true : false);

	const ctrlKey = (OS === OperatingSystem.Macintosh ? winCtrl : ctrlCmd);
	const shiftKey = (keybinding & BinaryKeybindingsMask.Shift ? true : false);
	const altKey = (keybinding & BinaryKeybindingsMask.Alt ? true : false);
	const metaKey = (OS === OperatingSystem.Macintosh ? ctrlCmd : winCtrl);
	const keyCode = (keybinding & BinaryKeybindingsMask.KeyCode);

	return new KeyCodeChord(ctrlKey, shiftKey, altKey, metaKey, keyCode);
}

export interface Modifiers {
}

/**
 * Represents a chord which uses the `keyCode` field of keyboard events.
 * A chord is a combination of keys pressed simultaneously.
 */
export class KeyCodeChord implements Modifiers {

	constructor(
		public readonly ctrlKey: boolean,
		public readonly shiftKey: boolean,
		public readonly altKey: boolean,
		public readonly metaKey: boolean,
		public readonly keyCode: KeyCode
	) { }
}

/**
 * Represents a chord which uses the `code` field of keyboard events.
 * A chord is a combination of keys pressed simultaneously.
 */
export class ScanCodeChord implements Modifiers {

	constructor(
		public readonly ctrlKey: boolean,
		public readonly shiftKey: boolean,
		public readonly altKey: boolean,
		public readonly metaKey: boolean,
		public readonly scanCode: ScanCode
	) { }
}

export type Chord = KeyCodeChord | ScanCodeChord;

/**
 * A keybinding is a sequence of chords.
 */
export class Keybinding {

	public readonly chords: Chord[];

	constructor(chords: Chord[]) {
		if (chords.length === 0) {
			throw illegalArgument(`chords`);
		}
		this.chords = chords;
	}
}

/**
 * A resolved keybinding. Consists of one or multiple chords.
 */
export abstract class ResolvedKeybinding {
	/**
	 * This prints the binding in a format suitable for displaying in the UI.
	 */
	public abstract getLabel(): string | null;
	/**
	 * This prints the binding in a format suitable for electron's accelerators.
	 * See https://github.com/electron/electron/blob/master/docs/api/accelerator.md
	 */
	public abstract getElectronAccelerator(): string | null;
}
