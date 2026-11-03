// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { Event } from '../../../../../base/common/event.js';
import type { DesktopCursor } from '../../../../../platform/engine/common/dataChannel.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { createWebCodecsDecoder } from '../../../../services/camera/webCodecsDecoder.js';
import { EngineDesktopSession } from '../../../../services/remoteDesktop/engineDesktopSession.js';
import still from './fixtures/still.json' with { type: 'json' };
import { DesktopTab, RemoteDesktopTabFixture } from './remoteDesktopTabFixture.js';

function bytes(base64: string): Uint8Array {
	return Uint8Array.from(atob(base64), character => character.charCodeAt(0));
}

/** PSNR of two RGBA pictures over their colour channels. */
function psnr(left: Uint8Array | Uint8ClampedArray, right: Uint8Array | Uint8ClampedArray): number {
	let sum = 0;
	let count = 0;
	for (let at = 0; at < left.length; at += 4) {
		for (let channel = 0; channel < 3; channel++) {
			const error = left[at + channel] - right[at + channel];
			sum += error * error;
			count++;
		}
	}
	return 10 * Math.log10(255 * 255 / Math.max(sum / count, 1e-9));
}

/**
 * A still picture as the host's `StillEncoder` makes it (AV1 4:4:4, in
 * `fixtures/still.json`, written by the `write_still_fixture` Rust test of
 * `vnilink-media`), shown as the app shows one: the engine session's own
 * stills decoder through WebCodecs, then the remote desktop view's canvas.
 */
suite('RemoteDesktopView stills', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	test('a coloured glyph keeps its colour far better than 4:2:0 keeps it', async () => {
		const fixture = disposables.add(new RemoteDesktopTabFixture(control => {
			const session = new EngineDesktopSession(control, { id: '4', generation: '2' }, createWebCodecsDecoder, () => Promise.resolve([]));
			session.attach({
				video: {
					onCursor: Event.None as Event<DesktopCursor>,
					requestKeyframe: () => undefined,
					mediaStatus: () => Promise.resolve(undefined),
					desktopAudio: () => Promise.reject(new Error('No sound in this test')),
					close: () => undefined,
				}, input: undefined,
			});
			return session;
		}));
		const tab = new DesktopTab(fixture.open(false), fixture.contextMenu);
		await fixture.opened(1);
		const metadata = { codec: 'av1', keyframe: true, timestampUs: '1', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0, role: 'still' };
		await fixture.sessions[0].consume({ type: 'data', sequence: 1, kind: 'video', data: bytes(still.still), metadata: JSON.stringify(metadata) });
		await tab.when(() => tab.canvas.width === still.width && tab.canvas.height === still.height);
		const { x, y, width, height } = still.crop;
		const drawn = tab.canvas.getContext('2d')?.getImageData(x, y, width, height).data;
		assert.ok(drawn, 'The screen cannot be read back');
		const kept = psnr(bytes(still.glyph), drawn);
		assert.ok(kept > still.subsampledPsnr + 6, `The glyph kept ${kept.toFixed(1)} dB, against ${String(still.subsampledPsnr)} dB at 4:2:0`);
	});
});
