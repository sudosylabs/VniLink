// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { DeviceFixtureState } from '../../smoke/fixtures/devices.js';
import type { Code } from './code.js';

const FIXTURE = '.device-fixture';

const PANEL = `${FIXTURE} .device-panel`;

const LIST = `${FIXTURE} .device-list`;

const POPOVER = '.device-pairing-popover';

const CODE = `${POPOVER} .device-pairing-code:not([hidden])`;

const DIGITS = `${POPOVER} .device-pairing-field:not([hidden]) .code-input-digit`;

const SELECTED_ROW = `${LIST} .monaco-list-row[aria-selected="true"]`;

const INSPECTOR_TITLE = `${FIXTURE} .device-panel-identity h2`;

const EMPTY_TITLE = `${FIXTURE} .device-empty h2`;

/** The selected computer's list row and the computer the inspector shows. */
export interface Selection {
	readonly rows: readonly string[];
	readonly inspector: string;
}

/** How the inspector sits beside, or over, the device list. */
export interface InspectorSheet {
	readonly scrolls: boolean;
	readonly listStays: boolean;
	readonly pageScrolls: boolean;
	readonly fullHeight: boolean;
	readonly atRightEdge: boolean;
	readonly overList: boolean;
}

/** The Devices page fixture and its pairing popover. */
export class Devices {

	constructor(private readonly code: Code) { }

	public async show(state: DeviceFixtureState): Promise<void> {
		await (await this.code.driver.fixtures()).showDeviceFixture(state);
		await this.code.driver.renderer.whenRendered();
	}

	public async clear(): Promise<void> {
		await (await this.code.driver.fixtures()).clearDeviceFixture();
	}

	public async inspector(): Promise<InspectorSheet> {
		const renderer = this.code.driver.renderer;
		const before = (await renderer.getLayout(LIST)).top;
		const scrolls = await renderer.scrollTo(PANEL, Number.MAX_SAFE_INTEGER) > 0;
		const listStays = (await renderer.getLayout(LIST)).top === before;
		await renderer.scrollTo(PANEL, 0);
		const host = await renderer.getLayout(FIXTURE);
		const panel = await renderer.getLayout(PANEL);
		const list = await renderer.getLayout(LIST);
		return {
			scrolls, listStays,
			pageScrolls: host.scrollHeight > host.clientHeight,
			fullHeight: Math.abs(panel.top - host.top) < 1 && Math.abs(panel.bottom - host.bottom) < 1,
			atRightEdge: Math.abs(panel.right - host.right) < 1,
			overList: panel.left < list.right,
		};
	}

	/** The text of each selected list row, and the title the inspector shows. */
	public async selection(): Promise<Selection> {
		const rows = await this.code.waitForElements(SELECTED_ROW, false, found => found.length > 0);
		const inspector = await this.code.waitForTextContent(INSPECTOR_TITLE);
		return { rows: rows.map(row => row.textContent), inspector };
	}

	/** Waits for the page with no computers to show `title`. */
	public async waitForEmptyTitle(title: string): Promise<void> {
		await this.code.waitForTextContent(EMPTY_TITLE, title);
	}

	public async overflows(): Promise<boolean> {
		const page = await this.code.driver.renderer.getLayout(`${FIXTURE} main`);
		return page.scrollWidth > page.clientWidth;
	}

	/** Creates an invitation from the empty page, downloads it, and returns the code it shows. */
	public async shareInvitation(): Promise<string> {
		const fixtures = await this.code.driver.fixtures();
		await fixtures.showDeviceFixture('empty');
		await fixtures.shareFixtureInvitation();
		const code = await this.code.waitForTextContent(CODE, undefined, undefined, 30);
		await this.code.waitAndClick(`${POPOVER} .monaco-button.primary`);
		return code;
	}

	/** A computer asks to pair, and its six digits are typed and confirmed. */
	public async approveIncoming(digits: string): Promise<void> {
		await (await this.code.driver.fixtures()).requestFixturePairing();
		await this.code.waitForElements(DIGITS, false, boxes => boxes.length === 6, 30);
		if ((await this.code.getElements(CODE, false) ?? []).length > 0) {
			throw new Error('Incoming confirmation disclosed the expected code');
		}
		await this.typeDigits(digits);
		await this.code.driver.renderer.sendKey('Enter', 13, DIGITS, 5);
	}

	public async waitForTitle(title: string): Promise<void> {
		await this.code.waitForTextContent(`${POPOVER} h2`, title, undefined, 30);
	}

	/** Opens the popover that opens an invitation, as the page's button does. */
	public async startOpeningInvitation(): Promise<void> {
		await this.code.driver.renderer.pointerDownOutside();
		await (await this.code.driver.fixtures()).importFixtureInvitation();
		await this.code.driver.renderer.whenRendered();
	}

	/** Opens an invitation file and types the first of the creator's digits. */
	public async openInvitation(digits: string): Promise<void> {
		await (await this.code.driver.fixtures()).openFixtureInvitationFile();
		await this.code.waitForElements(DIGITS, false, boxes => boxes.length === 6, 30);
		await this.typeDigits(digits);
		await this.code.driver.renderer.whenRendered();
	}

	/** Types the last digit, confirms, and returns the computer's own code once it shows. */
	public async finishInvitation(digit: string): Promise<string> {
		await this.code.driver.renderer.insertText(DIGITS, digit, 5);
		await this.code.driver.renderer.sendKey('Enter', 13, DIGITS, 5);
		return this.code.waitForTextContent(CODE, undefined, undefined, 30);
	}

	private async typeDigits(digits: string): Promise<void> {
		for (const [index, digit] of Array.from(digits).entries()) {
			await this.code.driver.renderer.focus(DIGITS, index);
			await this.code.driver.renderer.insertText(DIGITS, digit, index);
		}
	}
}
