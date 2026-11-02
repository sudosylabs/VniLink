// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isChannelRequest, isDesktopAudioOperation, isDesktopAudioStatus, isDesktopCursor } from '../../common/dataChannel.js';

suite('Data channel guards', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	suite('isChannelRequest', () => {
		const video = { kind: 'video', requestId: 'test', sessionId: '1', generation: '2' };
		const camera = { requestId: 'media', kind: 'camera', sessionId: '1', generation: '1', media: { type: 'camera', deviceId: 'camera-1', width: 1280, height: 720, fps: 30 } };
		const audio = { ...camera, kind: 'audio', media: { type: 'audio', deviceId: 'mic-1', sourceKind: 'Microphone', channels: 1, voice: true } };
		const accepted: [string, unknown][] = [
			['a video request', video],
			['a terminal of 24 by 80', { ...video, kind: 'terminal', rows: 24, columns: 80 }],
			['a snapshot that may neither prompt nor ask for access', { ...video, kind: 'snapshot', media: { type: 'snapshot', prompt: false, access: false } }],
			['a snapshot that may prompt and ask for access', { ...video, kind: 'snapshot', media: { type: 'snapshot', prompt: true, access: true } }],
			['a camera within its bounds', camera],
			['a microphone', audio],
		];
		for (const [name, request] of accepted) {
			test(`${name} is accepted`, () => {
				assert.strictEqual(isChannelRequest(request), true);
			});
		}
		const refused: [string, unknown][] = [
			['a kind that is not a string', { ...video, kind: ['video'] }],
			['a numeric session', { ...video, sessionId: 1 }],
			['a generation beyond 64 bits', { ...video, generation: '18446744073709551616' }],
			['an extra field', { ...video, extra: true }],
			['a terminal with no rows', { ...video, kind: 'terminal', rows: 0, columns: 80 }],
			['a snapshot without its media', { ...video, kind: 'snapshot' }],
			['a snapshot that does not say whether it may ask for access', { ...video, kind: 'snapshot', media: { type: 'snapshot', prompt: false } }],
			['a snapshot carrying a camera', { ...video, kind: 'snapshot', media: { type: 'camera', deviceId: 'x', width: 640, height: 360, fps: 5 } }],
			['a camera without its media', { ...camera, media: undefined }],
			['camera media on an audio channel', { ...camera, kind: 'audio' }],
			['camera media on a video channel', { ...camera, kind: 'video' }],
			['a camera too wide', { ...camera, media: { ...camera.media, width: 8192 } }],
			['a camera of a width it does not offer', { ...camera, media: { ...camera.media, width: 1279 } }],
			['a camera too fast', { ...camera, media: { ...camera.media, fps: 61 } }],
			['a camera naming another computer', { ...camera, media: { ...camera.media, endpointId: 'other' } }],
			['a microphone with three channels', { ...audio, media: { ...audio.media, channels: 3 } }],
			['an audio source of an unknown kind', { ...audio, media: { ...audio.media, sourceKind: 'output' } }],
		];
		for (const [name, request] of refused) {
			test(`${name} is refused`, () => {
				assert.strictEqual(isChannelRequest(request), false);
			});
		}
	});
	suite('desktop audio', () => {
		const status = { phase: 'unavailable', enabled: true, muted: false, playbackFailed: true, error: { code: 'Unsupported', message: 'No output' } };
		for (const operation of [{ type: 'status' }, { type: 'muted', muted: true }, { type: 'enabled', enabled: false }]) {
			test(`the operation ${JSON.stringify(operation)} is accepted`, () => {
				assert.strictEqual(isDesktopAudioOperation(operation), true);
			});
		}
		for (const operation of [{ type: 'status', sessionId: 'another' }, { type: 'muted', muted: 1 }, { type: 'enabled', enabled: true, muted: true }]) {
			test(`the operation ${JSON.stringify(operation)} is refused`, () => {
				assert.strictEqual(isDesktopAudioOperation(operation), false);
			});
		}
		test('a status with its phase, settings and error is accepted', () => {
			assert.strictEqual(isDesktopAudioStatus(status), true);
		});
		const refused: [string, unknown][] = [
			['an invented phase', { ...status, phase: 'invented' }],
			['no word of whether playback failed', { ...status, playbackFailed: undefined }],
			['decoded audio', { ...status, data: new Uint8Array([1]) }],
			['an error code too long', { ...status, error: { ...status.error, code: 'x'.repeat(65) } }],
		];
		for (const [name, value] of refused) {
			test(`a status with ${name} is refused`, () => {
				assert.strictEqual(isDesktopAudioStatus(value), false);
			});
		}
	});
	suite('isDesktopCursor', () => {
		const image = { width: 2, height: 1, hotspotX: 1, hotspotY: 0, pixels: new Uint8Array(8) };
		test('a pointer with an image it fills is accepted', () => {
			assert.strictEqual(isDesktopCursor({ x: 5, y: 6, visible: true, image }), true);
		});
		test('a hot spot outside the image is refused', () => {
			assert.strictEqual(isDesktopCursor({ x: 5, y: 6, visible: true, image: { ...image, hotspotX: 2 } }), false);
		});
		test('pixels that do not fill the image are refused', () => {
			assert.strictEqual(isDesktopCursor({ x: 5, y: 6, visible: true, image: { ...image, pixels: new Uint8Array(4) } }), false);
		});
	});
});
