// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The latency stimulus: a page that draws, on every animation frame, a strip
 * of cells carrying that frame's time in ms. The layout is the one
 * `vnilink_media::measure::timecode` reads: header 1 0 1 0, 20 value bits
 * most significant first, the same bits inverted, then 1 1, dark = 1, with a
 * light quiet zone of two cells or more either side.
 */

/** Cells in one strip, quiet zones excluded. */
export const CELLS = 46;

const VALUE_BITS = 20;

/** The strip for `value`, dark = `true`. */
export function cells(value: number): boolean[] {
	const masked = value & ((1 << VALUE_BITS) - 1);
	const strip = [true, false, true, false];
	const bits: boolean[] = [];
	for (let bit = VALUE_BITS - 1; bit >= 0; bit--) {
		bits.push(((masked >> bit) & 1) === 1);
	}
	return [...strip, ...bits, ...bits.map(set => !set), true, true];
}

/**
 * Hold mode (`measure:latency --hold`): the strip moves for `moveMs`, then
 * holds for `holdMs`, over and over from `origin` (`Date.now()`), and a
 * panel of coloured text fills the page below it. Holding, the page draws
 * nothing, so the screen does not change: the host's still picture comes,
 * and the first change after it is the one a still may hold up. Coloured
 * text is what a still is for: 4:2:0 blurs it, and the host sends a still
 * only when it beats 4:2:0.
 */
export interface StimulusHold {
	readonly origin: number;
	readonly moveMs: number;
	readonly holdMs: number;
}

/**
 * A self-contained page drawing the strip at `cell` CSS pixels a cell. With
 * `busy`, the page below the strip fills with scattered coloured blocks that
 * change on every frame, a picture an encoder spends its whole bitrate on
 * and a screen of that much detail changing does not; without it, the strip
 * is nearly all that changes and a stream uses a fraction of its bitrate.
 */
export function stimulusPage(cell: number, hold?: StimulusHold, busy = false): string {
	// The page cannot import, so it carries its own copy of `cells`;
	// `test/tooling/latency.test.mjs` keeps the copies and the reader alike.
	const script = `
const VALUE_BITS = ${String(VALUE_BITS)};
function cells(value) {
	const masked = value & ((1 << VALUE_BITS) - 1);
	const bits = [];
	for (let bit = VALUE_BITS - 1; bit >= 0; bit--) {
		bits.push(((masked >> bit) & 1) === 1);
	}
	return [true, false, true, false, ...bits, ...bits.map(set => !set), true, true];
}
const canvas = document.querySelector('canvas');
const context = canvas.getContext('2d', { alpha: false });
const scale = window.devicePixelRatio;
const cell = ${String(cell)};
const hold = ${JSON.stringify(hold ?? null)};
canvas.width = Math.round(innerWidth * scale);
canvas.height = Math.round(innerHeight * scale);
const busy = ${JSON.stringify(busy)};
const band = hold || busy ? Math.round(cell * 2 * scale) : canvas.height;
function holding() {
	return hold !== null && (Date.now() - hold.origin) % (hold.moveMs + hold.holdMs) >= hold.moveMs;
}
// Holding, the page asks for no frames at all until it moves again: a
// window that only asks for one may present a new picture anyway.
function resume() {
	const cycle = hold.moveMs + hold.holdMs;
	return cycle - (Date.now() - hold.origin) % cycle;
}
let seed = 7;
const next = () => (seed = (seed * 1103515245 + 12345) % 2147483648);
if (busy) {
	context.fillStyle = '#fff';
	context.fillRect(0, band, canvas.width, canvas.height - band);
}
if (hold) {
	// Coloured words on white, the same on every run.
	const colours = ['#c62828', '#1565c0', '#2e7d32', '#ef6c00', '#6a1b9a', '#00838f'];
	const words = ['const', 'value', 'stream', 'picture', 'colour', 'still', 'return', 'match', 'frame', 'link'];
	const size = Math.round(14 * scale);
	context.fillStyle = '#fff';
	context.fillRect(0, band, canvas.width, canvas.height - band);
	context.font = String(size) + 'px Menlo, monospace';
	context.textBaseline = 'top';
	// The top half: the viewer covers the bottom half.
	for (let y = band + size; y < canvas.height / 2 - size; y += Math.round(size * 1.5)) {
		for (let x = size; x < canvas.width - size * 8;) {
			const word = words[next() % words.length];
			context.fillStyle = colours[next() % colours.length];
			context.fillText(word, x, y);
			x += context.measureText(word + ' ').width;
		}
	}
}
function draw(time) {
	if (holding()) {
		setTimeout(() => requestAnimationFrame(draw), resume());
		return;
	}
	const strip = cells(Math.round(time));
	context.fillStyle = '#fff';
	context.fillRect(0, 0, canvas.width, band);
	context.fillStyle = '#000';
	const top = Math.round(cell * 0.25 * scale);
	const height = Math.round(cell * 1.5 * scale);
	strip.forEach((dark, index) => {
		if (dark) {
			const left = Math.round((2 + index) * cell * scale);
			const right = Math.round((3 + index) * cell * scale);
			context.fillRect(left, top, right - left, height);
		}
	});
	if (busy) {
		// Several hundred blocks a frame, the same sequence on every run.
		for (let block = 0; block < 400; block++) {
			// The low bits of this generator repeat, so the high ones are used.
			const random = limit => (next() >>> 8) % limit;
			context.fillStyle = 'rgb(' + random(256) + ',' + random(256) + ',' + random(256) + ')';
			const size = 8 + random(56);
			context.fillRect(random(canvas.width), band + random(canvas.height - band), size, size);
		}
	}
	requestAnimationFrame(draw);
}
requestAnimationFrame(draw);`;
	return `<!doctype html><meta charset="utf-8"><title>Latency stimulus</title>
<style>html,body{margin:0;height:100%;background:#fff;overflow:hidden}canvas{display:block;width:100%;height:100%}</style>
<canvas></canvas><script>${script}</script>`;
}
