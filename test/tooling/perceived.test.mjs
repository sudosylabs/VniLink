// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { Script } from 'node:vm';
import { join, resolve } from 'node:path';
import { cardReference, LOCAL_LABEL, perceivedOptions, perceivedUsage, renderPerceived, summarisePerceived } from '../../build/lib/perceived.ts';
import { perceivedCard, perceivedLayout, perceivedPage } from '../../test/perceived/stimulus.ts';

test('by default the page runs on this Mac and the run is named for it', () => {
	const options = perceivedOptions('/root', []);
	assert.deepEqual(Object.keys(options).sort(), ['label', 'output', 'peer', 'waitSeconds']);
	assert.equal(options.peer, undefined);
	assert.equal(options.label, LOCAL_LABEL);
	assert.equal(options.waitSeconds, 300);
	assert.ok(options.output.startsWith(resolve('/root', '.build', 'perceived', 'This-Mac-')));
	assert.equal(perceivedOptions('/root', ['--label', 'Studio display']).label, 'Studio display');
	// The second computer's options mean nothing without it.
	assert.throws(() => perceivedOptions('/root', ['--peer-check', '123']), /go together/);
	assert.throws(() => perceivedOptions('/root', ['--peer', 'agent.vnl', '--label', 'x']), /go together/);
	assert.throws(() => perceivedOptions('/root', ['--label', '']), /--label/);
	assert.throws(() => perceivedOptions('/root', ['--wait', '1801']), /--wait/);
	assert.equal(perceivedOptions('/root', ['--wait', '10']).waitSeconds, 10);
	assert.throws(() => perceivedOptions('/root', ['--peer']));
	assert.throws(() => perceivedOptions('/root', ['--unknown']));
	// SSH is gone: its options are unknown.
	assert.throws(() => perceivedOptions('/root', ['--remote', 'host', '--remote-root', '/r', '--label', 'x']));
	// The usage leads with the one-Mac run; the second computer is optional.
	assert.ok(perceivedUsage.indexOf('By default the page runs on this Mac') < perceivedUsage.indexOf('Optional, a second computer'));
	assert.match(perceivedUsage, /--label <name> {10}required with --peer/);
});

test('perceived options name the agent on the controlled computer and the application', () => {
	const peer = ['--peer', 'agent.vnl', '--peer-check', '123'];
	const options = perceivedOptions('/root', [...peer, '--label', 'VniLink']);
	assert.deepEqual(options.peer, { file: resolve('agent.vnl'), check: '123' });
	assert.equal(options.waitSeconds, 300);
	assert.ok(options.output.startsWith(resolve('/root', '.build', 'perceived', 'VniLink-')));
	assert.throws(() => perceivedOptions('/root', ['--peer', 'agent.vnl', '--peer-check', '1234', '--label', 'x']), /three digits/);
	assert.throws(() => perceivedOptions('/root', peer), /--label names the application under test/);
	assert.throws(() => perceivedOptions('/root', [...peer, '--label', 'a;b']), /--label/);
	assert.throws(() => perceivedOptions('/root', [...peer, '--label', 'x', '--wait', '5']), /--wait/);
	assert.equal(perceivedOptions('/root', [...peer, '--label', 'x', '--output', 'o']).output, join(process.cwd(), 'o'));
});

test('the layout matches the reader: the card sits where crates/vnilink-media reads it', () => {
	// `measure::perceived::card(1920, 1080, 12)` in the Rust tests.
	assert.deepEqual(perceivedLayout(1920, 1080, 1).card, { x: 672, y: 24, width: 1024, height: 768 });
	assert.equal(perceivedLayout(3840, 2160, 2).cell, 24);
	assert.deepEqual(perceivedLayout(1920, 1080, 1).anchorBottomRight, { x: 1296, y: 1020, width: 600, height: 36 });
});

test('the card is deterministic, inside its bounds, and its reference is RGB at its size', () => {
	for (const variant of [0, 1]) {
		const rects = perceivedCard(variant, 1024, 768);
		assert.deepEqual(rects, perceivedCard(variant, 1024, 768));
		assert.ok(rects.every(rect => rect.x >= 0 && rect.y >= 0 && rect.x + rect.width <= 1024 && rect.y + rect.height <= 768));
	}
	assert.notDeepEqual(perceivedCard(0, 1024, 768), perceivedCard(1, 1024, 768));
	const reference = cardReference(1, 64, 48);
	assert.equal(reference.length, 64 * 48 * 3);
	// Variant 1 is on dark paper.
	assert.deepEqual([...reference.subarray(0, 3)], [24, 28, 48]);
	const page = perceivedPage();
	new Script(page.split('<script>')[1].split('</script>')[0]);
});

test('a run is summarised from what the viewer screen showed', () => {
	const line = value => JSON.stringify(value);
	const frame = (t, fields) => line({ type: 'frame', t, frame: Math.round(t / 16.7), clicks: 0, x: 0, y: 0, card: 0, ...fields });
	const events = [
		// A first lock the picture moved away from: its events do not count.
		line({ type: 'located', t: -900, scaleX: 1, scaleY: 1, cell: 12, hostWidth: 1920, hostHeight: 1080, viewerWidth: 3840, viewerHeight: 2160 }),
		line({ type: 'click', t: -800 }),
		line({ type: 'moved', t: -500 }),
		line({ type: 'located', t: 0, scaleX: 1.6, scaleY: 1.6, cell: 12, hostWidth: 1920, hostHeight: 1080, viewerWidth: 3840, viewerHeight: 2160 }),
		line({ type: 'move', kind: 'jump', t: 100, hostX: 500, hostY: 300 }),
		frame(130, { x: 0, y: 0 }),
		frame(160, { x: 500, y: 301 }),
		line({ type: 'move', kind: 'jump', t: 400, hostX: 900, hostY: 600 }),
		frame(450, { x: 900, y: 600 }),
		line({ type: 'move', kind: 'path', t: 800, hostX: 10, hostY: 10 }),
		line({ type: 'move', kind: 'path', t: 808, hostX: 20, hostY: 20 }),
		frame(850, { x: 10, y: 10 }),
		frame(870, { x: 20, y: 20 }),
		line({ type: 'click', t: 1500 }),
		frame(1490, { clicks: 0 }),
		frame(1560, { clicks: 1 }),
		// Never seen: the count stays at one.
		line({ type: 'click', t: 1900 }),
		// Two keys typed and both shown; one more in a burst that never shows.
		line({ type: 'key', phase: 'paced', t: 2000, keys: 1, typed: 11 }),
		frame(2040, { clicks: 1, keys: 1, typed: 11 }),
		line({ type: 'key', phase: 'paced', t: 2100, keys: 2, typed: 22 }),
		frame(2190, { clicks: 1, keys: 2, typed: 22 }),
		line({ type: 'key', phase: 'burst', t: 2300, keys: 3, typed: 33 }),
		frame(2310, { clicks: 1, keys: 2, typed: 22 }),
		...Array.from({ length: 61 }, (_, index) => frame(2500 + index * 16.7, { clicks: 1, card: index > 30 ? 1 : 0 })),
		line({ type: 'crop', t: 3020, card: 1, sinceMs: 0, file: 'crop-0.bgra', width: 8, height: 8 }),
		line({ type: 'crop', t: 3270, card: 1, sinceMs: 250, file: 'crop-1.bgra', width: 8, height: 8 }),
		line({ type: 'crop', t: 9520, card: 1, sinceMs: 6500, file: 'crop-2.bgra', width: 8, height: 8 }),
		// Frozen: the counter stops, and later pictures of the card are sharper.
		line({ type: 'freeze', t: 10000 }),
		...Array.from({ length: 30 }, (_, index) => frame(10000 + index * 100, { clicks: 1, card: 1, frame: 999 })),
		line({ type: 'crop', phase: 'still', t: 10050, card: 1, sinceMs: 50, file: 'crop-3.bgra', width: 8, height: 8 }),
		line({ type: 'crop', phase: 'still', t: 12400, card: 1, sinceMs: 2400, file: 'crop-4.bgra', width: 8, height: 8 }),
	].join('\n');
	const ssim = new Map([['crop-0.bgra', 0.7], ['crop-1.bgra', 0.95], ['crop-2.bgra', 0.955], ['crop-3.bgra', 0.8], ['crop-4.bgra', 0.99]]);
	const summary = summarisePerceived('Test', events, ssim);
	assert.equal(summary.scale, 1.6);
	assert.equal(summary.restarts, 1);
	assert.deepEqual([summary.pointerJump?.p50, summary.pointerJump?.count, summary.pointerMissed], [50, 2, 0]);
	assert.equal(summary.pointerError?.max, 1);
	assert.deepEqual([summary.pointerTrail?.p50, summary.pointerTrail?.max], [50, 62]);
	assert.deepEqual([summary.click?.p50, summary.clicksMissed], [60, 1]);
	assert.ok(Math.abs((summary.framesPerSecond ?? 0) - 59.9) < 0.2);
	assert.equal(summary.stutters, 0);
	assert.deepEqual([summary.settledSsim, summary.sharpAfterMs], [0.955, 250]);
	// Still pictures are their own measure, and the frozen page is not a stutter.
	assert.deepEqual([summary.frozen, summary.stillSsim, summary.stillSharpAfterMs, summary.stutters], [true, 0.99, 2400, 0]);
	assert.equal(summary.sharpness.length, 3);
	assert.deepEqual([summary.keys?.pressed, summary.keys?.shown, summary.keys?.textRight, summary.keys?.latency?.p50], [2, 2, true, 40]);
	assert.deepEqual([summary.keyBurst?.pressed, summary.keyBurst?.shown, summary.keyBurst?.textRight], [1, 0, false]);
	const text = renderPerceived([summary, { ...summary, label: 'Other' }]);
	assert.match(text, /\| \| Test \| Other \|/);
	assert.match(text, /\| Card sharp after a swap, ms \| 250 \| 250 \|/);
	assert.match(text, /\| Picture moved, measurement restarted \| 1 \| 1 \|/);
	assert.match(text, /\| Still screen: card SSIM first \/ last \| 0\.8000 \/ 0\.9900 \|/);
	assert.match(text, /\| Keys shown \/ pressed, text right \| 2 \/ 2, yes \|/);
});
