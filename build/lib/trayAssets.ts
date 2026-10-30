// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Renders the tray icons from the product mark. The PNGs are committed like
 * the other artwork; run `node build/run.ts assets:tray` after changing the
 * mark or these compositions. Rendering needs `rsvg-convert` (librsvg).
 */
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);

export type TrayState = 'idle' | 'active' | 'paused';

export const trayStates: readonly TrayState[] = ['idle', 'active', 'paused'];

/** The connected dot on Windows and Linux; macOS template images take the menu bar's tint. */
const accent = '#547eea';

/** Badge geometry in the mark's own coordinates (`viewBox="40 24 312 312"`), sized to read at 16 pixels. */
const badge = { x: 290, y: 262, radius: 52, gap: 24 };

interface Variant {
	/** The file name before the scale suffix. */
	readonly name: (state: TrayState) => string;
	readonly glyph: string;
	readonly dot: string;
	/** Logical size and the scales Electron loads as `@<scale>x` representations. */
	readonly size: number;
	readonly scales: readonly number[];
}

/** macOS template images, and glyphs for dark and light Windows/Linux panels. */
export const trayVariants: readonly Variant[] = [
	{ name: state => `${state}Template`, glyph: '#000000', dot: '#000000', size: 16, scales: [1, 2] },
	{ name: state => `${state}-dark`, glyph: '#ffffff', dot: accent, size: 16, scales: [1, 1.5, 2] },
	{ name: state => `${state}-light`, glyph: '#000000', dot: accent, size: 16, scales: [1, 1.5, 2] },
];

/** The file a variant's state renders to at `scale`, as `nativeImage` looks it up. */
export function trayFileName(variant: Variant, state: TrayState, scale: number): string {
	return `${variant.name(state)}${scale === 1 ? '' : `@${String(scale)}x`}.png`;
}

/** One state's SVG: the mark, with a badge cut out of it for `active` and `paused`. */
export function traySvg(markPath: string, state: TrayState, variant: Variant): string {
	const { x, y, radius, gap } = badge;
	const cut = state === 'idle' ? '' : `<mask id="cut"><rect x="40" y="24" width="312" height="312" fill="#fff"/><circle cx="${String(x)}" cy="${String(y)}" r="${String(radius + gap)}" fill="#000"/></mask>`;
	const mark = `<path d="${markPath}" fill="${variant.glyph}"${state === 'idle' ? '' : ' mask="url(#cut)"'}${state === 'paused' ? ' fill-opacity="0.45"' : ''}/>`;
	const bar = { width: radius * 0.42, height: radius * 1.3 };
	const marker = state === 'active'
		? `<circle cx="${String(x)}" cy="${String(y)}" r="${String(radius)}" fill="${variant.dot}"/>`
		: state === 'paused'
			? [-1, 1].map(side => `<rect x="${String(x + side * bar.width * 0.85 - bar.width / 2)}" y="${String(y - bar.height / 2)}" width="${String(bar.width)}" height="${String(bar.height)}" rx="${String(bar.width / 3)}" fill="${variant.glyph}"/>`).join('')
			: '';
	return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="40 24 312 312"><defs>${cut}</defs>${mark}${marker}</svg>\n`;
}

/** Renders every state and variant into `assets/tray`. */
export async function renderTrayAssets(root: string, signal: AbortSignal): Promise<void> {
	const source = await readFile(join(root, 'assets/tray/mark-black.svg'), 'utf8');
	// The mark is several paths; one combined path takes the badge mask and opacity as a whole.
	const markPath = [...source.matchAll(/\sd="([^"]+)"/g)].map(match => match[1]).join(' ');
	if (!markPath) {
		throw new Error('assets/tray/mark-black.svg has no path');
	}
	const work = await mkdtemp(join(tmpdir(), 'vnilink-tray-'));
	try {
		for (const variant of trayVariants) {
			for (const state of trayStates) {
				const svg = join(work, `${variant.name(state)}.svg`);
				await writeFile(svg, traySvg(markPath, state, variant));
				for (const scale of variant.scales) {
					const pixels = String(Math.round(variant.size * scale));
					try {
						await run('rsvg-convert', ['-w', pixels, '-h', pixels, '-o', join(root, 'assets/tray', trayFileName(variant, state, scale)), svg], { signal });
					} catch (error) {
						signal.throwIfAborted();
						throw new Error('Rendering tray icons needs rsvg-convert (librsvg: `brew install librsvg` or `apt install librsvg2-bin`).', { cause: error });
					}
				}
			}
		}
		console.log(`Tray icons rendered: ${String(trayVariants.reduce((total, variant) => total + trayStates.length * variant.scales.length, 0))} files in assets/tray`);
	} finally {
		await rm(work, { recursive: true, force: true });
	}
}
