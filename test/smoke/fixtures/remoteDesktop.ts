// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IDisposable } from '../../../src/base/common/lifecycle.js';
import type { DeviceSummary } from '../../../src/workbench/services/devices/deviceService.js';
import { TestDesktopSession } from '../../../src/workbench/contrib/remoteDesktop/test/browser/desktopViewFakes.js';
import { click, DesktopTab, RemoteDesktopTabFixture } from '../../../src/workbench/contrib/remoteDesktop/test/browser/remoteDesktopTabFixture.js';

/** What the desktop fixture shows: the line alone, its details, a slowed picture, or the controls brought down. */
export type DesktopFixtureState = 'collapsed' | 'details' | 'slow' | 'bar';

const device: DeviceSummary = { id: 'studio', name: 'Studio Mac', os: 'macos', presence: 'connected', hostname: 'studio.local', direct: 1, relays: 0 };

/** A desktop-like scene: a menu bar, a window with a sidebar and a terminal over a dusk background. */
function scene(): VideoFrame {
	const canvas = new OffscreenCanvas(1920, 1080);
	const context = canvas.getContext('2d');
	if (context) {
		const sky = context.createLinearGradient(0, 0, 1920, 1080);
		sky.addColorStop(0, '#3b4a6b');
		sky.addColorStop(1, '#1f2638');
		context.fillStyle = sky;
		context.fillRect(0, 0, 1920, 1080);
		context.fillStyle = 'rgba(30, 30, 34, 0.75)';
		context.fillRect(0, 0, 1920, 32);
		context.fillStyle = '#f4f4f6';
		context.fillRect(200, 140, 900, 560);
		context.fillStyle = '#e9e9ed';
		context.fillRect(200, 140, 220, 560);
		context.fillStyle = '#1e1e1e';
		context.fillRect(820, 460, 820, 480);
	}
	return new VideoFrame(canvas, { timestamp: 0 });
}

/**
 * A controlling remote desktop tab over a desktop-like picture, filling the
 * window below the titlebar, for the smoke run's screenshots. Synthetic: a
 * painted scene and test stats, not a captured screen.
 */
export async function showDesktopFixture(state: DesktopFixtureState): Promise<IDisposable> {
	const fixture = new RemoteDesktopTabFixture(control => {
		const session = new TestDesktopSession(control);
		session.conditions = state === 'slow' ? ['link.congestion', 'relay.path'] : [];
		session.reason = state === 'slow' ? 'networkPressure' : 'stable';
		return session;
	}, device, 'position:fixed;inset:var(--titlebar-height) 0 0 0;z-index:99;display:flex;');
	try {
		const tab = new DesktopTab(fixture.open(), fixture.contextMenu);
		await fixture.opened(1);
		fixture.sessions[0].firePicture(scene());
		await tab.when(() => tab.overlay.hidden === true);
		await tab.when(() => tab.text('.desktop-stats-summary') !== '');
		if (state === 'details') {
			click(tab.required('.desktop-stats-toggle'));
		}
		if (state === 'slow') {
			await tab.when(() => tab.required('.desktop-stats-condition').hidden === false);
		}
		if (state === 'bar') {
			const box = tab.element.getBoundingClientRect();
			tab.element.dispatchEvent(new PointerEvent('pointermove', { clientX: box.left + 400, clientY: box.top + 1, bubbles: true }));
		}
		return fixture;
	} catch (error) {
		fixture.dispose();
		throw error;
	}
}
