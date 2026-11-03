// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * What the smoke suite shows and runs inside the window, loaded as the
 * `browserTests` bundle: fixtures it lays out and photographs, and the one
 * renderer check that needs the real engine. Behaviour is tested by the unit
 * suites beside each module (`node build/run.ts test:unit:electron`).
 */
import type { IDisposable } from '../../../src/base/common/lifecycle.js';
import { ContextMenuFixture } from './contextMenu.js';
import { verifyDataChannels } from './dataChannels.js';
import { DevicePageFixture, type DeviceFixtureState } from './devices.js';
import { showThemedDialog } from './dialogs.js';
import { showFilesFixture } from './files.js';
import { showDesktopFixture, type DesktopFixtureState } from './remoteDesktop.js';

export class BrowserScenarios {
	private deviceFixture: DevicePageFixture | undefined;
	private desktopFixture: IDisposable | undefined;
	private contextMenuFixture: ContextMenuFixture | undefined;
	private filesFixture: IDisposable | undefined;

	public showDeviceFixture(state: DeviceFixtureState): void {
		this.clearDeviceFixture();
		this.deviceFixture = new DevicePageFixture(state);
	}

	/** Opens the fixture's create-invitation popover, as the page's button does. */
	public shareFixtureInvitation(): void {
		this.deviceFixture?.anchor('create').dispatchEvent(new MouseEvent('click', { bubbles: true }));
	}

	/** Opens the fixture's open-invitation popover, as the page's button does. */
	public importFixtureInvitation(): void {
		this.deviceFixture?.anchor('import').dispatchEvent(new MouseEvent('click', { bubbles: true }));
	}

	/** Opens an invitation file in the fixture, as choosing or dropping one does. */
	public async openFixtureInvitationFile(): Promise<void> {
		await this.deviceFixture?.popoverPairing.openInvitation(new File(['VNLF'], 'office-pc.vnl'));
	}

	public requestFixturePairing(): void {
		if (this.deviceFixture) {
			this.deviceFixture.incoming = { endpoint_id: 'test-computer' };
		}
	}

	public clearDeviceFixture(): void {
		this.deviceFixture?.dispose();
		this.deviceFixture = undefined;
	}

	/** Opens the themed restart confirmation, which Escape answers. */
	public showDialogFixture(): void {
		void showThemedDialog();
	}

	/** Opens a context menu; `closeContextMenuFixture` cancels it with Escape. */
	public showContextMenuFixture(): void {
		this.contextMenuFixture?.dispose();
		this.contextMenuFixture = new ContextMenuFixture();
		this.contextMenuFixture.show();
	}

	public closeContextMenuFixture(): void {
		for (const type of ['keydown', 'keyup']) {
			document.activeElement?.dispatchEvent(new KeyboardEvent(type, { key: 'Escape', code: 'Escape', keyCode: 27, bubbles: true }));
		}
		this.contextMenuFixture?.dispose();
		this.contextMenuFixture = undefined;
	}

	/** Shows the Files tab mid-task at `width`×`height`; `clearFilesFixture` removes it. */
	public async showFilesFixture(width: number, height: number): Promise<void> {
		this.clearFilesFixture();
		this.filesFixture = await showFilesFixture(width, height);
	}

	public clearFilesFixture(): void {
		this.filesFixture?.dispose();
		this.filesFixture = undefined;
	}

	/** Shows a controlling remote desktop tab in `state`; `clearDesktopFixture` removes it. */
	public async showDesktopFixture(state: DesktopFixtureState): Promise<void> {
		this.clearDesktopFixture();
		this.desktopFixture = await showDesktopFixture(state);
	}

	public clearDesktopFixture(): void {
		this.desktopFixture?.dispose();
		this.desktopFixture = undefined;
	}

	/**
	 * Runs the renderer part of smoke scenario `name`. `data-channels` checks
	 * the real engine's channels; the other scenarios only need these fixtures
	 * loaded, and the smoke suite lays them out and photographs them itself.
	 */
	public async run(name: string): Promise<boolean> {
		if (name === 'data-channels') {
			await verifyDataChannels();
		} else if (!['devices', 'dialogs', 'files', 'context-menu', 'remote-desktop-session'].includes(name)) {
			throw new Error(`Unknown smoke scenario: ${name}`);
		}
		return true;
	}
}

export const browserScenarios = new BrowserScenarios();
