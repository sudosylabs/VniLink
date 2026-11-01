// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { hidUsage } from '../../../platform/engine/common/keyUsage.js';
import type { DesktopInput } from '../../services/remoteDesktop/remoteDesktopService.js';

/** The HID usage of the left Control key, which Windows also presses for AltGr. */
const CONTROL_LEFT = 0xe0;

/**
 * Turns the keys pressed on this computer into the keys of the other one, and remembers which
 * it holds down.
 */
export class DesktopKeyboard {
	/** Keys sent down and not yet up, by usage. */
	private readonly down = new Set<number>();
	/**
	 * On Windows, AltGr arrives as a left Control press just before it. That
	 * Control is held back until the next key says which it was, and dropped
	 * with its release when it was AltGr's.
	 */
	private heldControl = false;
	private phantomControl = false;

	/**
	 * `send` carries a key to the other computer. `hold` says input is in progress that the other
	 * computer will need told to release, as a Control held back for AltGr is.
	 */
	constructor(
		private readonly send: (input: DesktopInput) => void,
		private readonly hold: () => void,
	) { }

	/**
	 * A key, by where it is on the keyboard, with the character it typed:
	 * the other computer types that character whatever its own layout.
	 * Shortcuts stay keys: no character travels with Control or Command, nor
	 * on Windows with Alt, whose shortcuts type nothing. AltGr types, and so
	 * does Option on a Mac, as both make characters there.
	 */
	public key(event: KeyboardEvent, down: boolean): void {
		const usage = hidUsage(event.code);
		if (usage === undefined) {
			return;
		}
		event.preventDefault();
		event.stopPropagation();
		if (isWindows) {
			if (down && event.code === 'ControlLeft' && !event.repeat && !this.down.has(CONTROL_LEFT)) {
				this.heldControl = true;
				this.hold();
				return;
			}
			if (this.heldControl) {
				this.heldControl = false;
				if (down && event.code === 'AltRight' && event.key === 'AltGraph') {
					this.phantomControl = true;
				} else {
					this.sendKey(CONTROL_LEFT, true, '', false);
				}
			}
			if (!down && event.code === 'ControlLeft' && this.phantomControl) {
				this.phantomControl = false;
				return;
			}
		}
		// Windows reports Print Screen only as it is let go.
		if (!down && !this.down.has(usage)) {
			this.sendKey(usage, true, '', false);
		}
		const altGr = event.getModifierState('AltGraph');
		const shortcut = event.metaKey || (event.ctrlKey && !altGr) || (!isMacintosh && event.altKey && !altGr);
		// One character, emoji included; named keys such as Enter are longer.
		const point = event.key.codePointAt(0);
		const character = point !== undefined && String.fromCodePoint(point) === event.key;
		const composing = down && (event.key === 'Dead' || event.key === 'Process' || event.isComposing);
		// A held key repeats as the other computer types it: typed characters don't repeat there by themselves.
		this.sendKey(usage, down, down && character && !shortcut ? event.key : '', composing);
	}

	/** A Control held back for AltGr goes down before anything that isn't a key, such as Control-click. */
	public flushControl(): void {
		if (this.heldControl) {
			this.heldControl = false;
			this.sendKey(CONTROL_LEFT, true, '', false);
		}
	}

	/** Forgets every key held; the caller tells the other computer to let go of them. */
	public release(): void {
		this.heldControl = false;
		this.phantomControl = false;
		this.down.clear();
	}

	private sendKey(usage: number, down: boolean, text: string, composing: boolean): void {
		if (down) {
			this.down.add(usage);
		} else {
			this.down.delete(usage);
		}
		this.send({ type: 'submitLocalKey', usage, down, text, composing });
	}
}
