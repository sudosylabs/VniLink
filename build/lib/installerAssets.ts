// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Renders the installers' artwork from the product mark: the macOS disk
 * image's background and the Windows setup's sidebar and header. The images
 * are committed like the other artwork; run `node build/run.ts assets:installers`
 * after changing the mark or these compositions. Rendering needs
 * `rsvg-convert` (librsvg); NSIS takes only 24-bit BMP files, which are
 * written here from the rendered PNG.
 */
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { inflateSync } from 'node:zlib';
import { processOutput } from './processes.ts';

/** The product's purple, from the application icon's gradient. */
const purple = { light: '#A855F7', dark: '#7C2AEF' };

/** The disk image window and the centres of its two icons, as `package.darwin.ts` lays them out. */
export const dmgLayout = { width: 540, height: 380, application: { x: 140, y: 170 }, applications: { x: 400, y: 170 } };

/** One rendered image: its path under `assets/`, its size and the scales it is rendered at. */
export interface InstallerImage {
	readonly path: string;
	readonly width: number;
	readonly height: number;
	readonly format: 'png' | 'bmp';
	readonly scales: readonly number[];
	readonly svg: (markPath: string) => string;
}

const fonts = 'font-family="-apple-system, \'Segoe UI\', \'Helvetica Neue\', Helvetica, Arial, sans-serif"';

/** The mark scaled into a square of `size` at `x`, `y`, from its own `viewBox="40 24 312 312"`. */
function mark(markPath: string, x: number, y: number, size: number, fill: string): string {
	const scale = size / 312;
	return `<path transform="translate(${String(x)} ${String(y)}) scale(${String(scale)}) translate(-40 -24)" d="${markPath}" fill="${fill}"/>`;
}

/** A pale page with an arrow from the application to the Applications folder and one line saying what to do. */
function dmgSvg(): string {
	const { width, height, application, applications } = dmgLayout;
	const from = application.x + 62;
	const to = applications.x - 62;
	const y = application.y;
	return [
		`<svg xmlns="http://www.w3.org/2000/svg" width="${String(width)}" height="${String(height)}" viewBox="0 0 ${String(width)} ${String(height)}">`,
		'<defs><linearGradient id="page" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FBFAFE"/><stop offset="1" stop-color="#F1EEF8"/></linearGradient></defs>',
		`<rect width="${String(width)}" height="${String(height)}" fill="url(#page)"/>`,
		// A gradient cannot fill the stroke of a line with no height, so the dots take the lighter purple.
		`<path d="M ${String(from)} ${String(y)} H ${String(to - 14)}" stroke="${purple.light}" stroke-width="6" stroke-linecap="round" stroke-dasharray="0.1 14"/>`,
		`<path d="M ${String(to - 18)} ${String(y - 14)} L ${String(to)} ${String(y)} L ${String(to - 18)} ${String(y + 14)}" fill="none" stroke="${purple.dark}" stroke-width="6" stroke-linecap="round" stroke-linejoin="round"/>`,
		`<text x="${String(width / 2)}" y="${String(height - 58)}" text-anchor="middle" ${fonts} font-size="15" fill="#4A4458">Drag VniLink to Applications to install it.</text>`,
		'</svg>\n',
	].join('');
}

/** The welcome and finish pages' sidebar: the mark in white on the product's purple. */
function sidebarSvg(markPath: string): string {
	return [
		'<svg xmlns="http://www.w3.org/2000/svg" width="164" height="314" viewBox="0 0 164 314">',
		`<defs><linearGradient id="fill" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${purple.light}"/><stop offset="1" stop-color="${purple.dark}"/></linearGradient></defs>`,
		'<rect width="164" height="314" fill="url(#fill)"/>',
		mark(markPath, 42, 96, 80, '#FFFFFF'),
		`<text x="82" y="214" text-anchor="middle" ${fonts} font-size="20" font-weight="600" fill="#FFFFFF">VniLink</text>`,
		'</svg>\n',
	].join('');
}

/** The other pages' header: the mark in purple on white, at the right where the header shows its image. */
function headerSvg(markPath: string): string {
	return [
		'<svg xmlns="http://www.w3.org/2000/svg" width="150" height="57" viewBox="0 0 150 57">',
		'<rect width="150" height="57" fill="#FFFFFF"/>',
		mark(markPath, 102, 10, 37, purple.dark),
		'</svg>\n',
	].join('');
}

export const installerImages: readonly InstallerImage[] = [
	{ path: 'apple/dmg/background.png', width: dmgLayout.width, height: dmgLayout.height, format: 'png', scales: [1, 2], svg: dmgSvg },
	{ path: 'windows/installer-sidebar.bmp', width: 164, height: 314, format: 'bmp', scales: [1], svg: sidebarSvg },
	{ path: 'windows/installer-header.bmp', width: 150, height: 57, format: 'bmp', scales: [1], svg: headerSvg },
];

/** The file an image renders to at `scale`, as macOS looks up a background's `@2x` representation. */
export function installerImageFile(image: InstallerImage, scale: number): string {
	return scale === 1 ? image.path : image.path.replace(/\.(\w+)$/, `@${String(scale)}x.$1`);
}

/** The pixels of an 8-bit RGB or RGBA, non-interlaced PNG such as rsvg-convert writes, as RGBA rows. */
export function decodePng(bytes: Buffer): { readonly width: number; readonly height: number; readonly rgba: Buffer } {
	if (bytes.subarray(1, 4).toString('latin1') !== 'PNG') {
		throw new Error('Not a PNG file');
	}
	let offset = 8;
	let width = 0;
	let height = 0;
	let channels = 0;
	const data: Buffer[] = [];
	while (offset < bytes.length) {
		const length = bytes.readUInt32BE(offset);
		const type = bytes.subarray(offset + 4, offset + 8).toString('latin1');
		const body = bytes.subarray(offset + 8, offset + 8 + length);
		if (type === 'IHDR') {
			width = body.readUInt32BE(0);
			height = body.readUInt32BE(4);
			const [depth, color, , , interlace] = [body[8], body[9], body[10], body[11], body[12]];
			if (depth !== 8 || (color !== 2 && color !== 6) || interlace !== 0) {
				throw new Error('Only 8-bit RGB or RGBA PNGs without interlacing are read');
			}
			channels = color === 6 ? 4 : 3;
		} else if (type === 'IDAT') {
			data.push(body);
		}
		offset += 12 + length;
	}
	const raw = inflateSync(Buffer.concat(data));
	const stride = width * channels;
	const rows = Buffer.alloc(stride * height);
	for (let y = 0; y < height; y++) {
		const filter = raw[y * (stride + 1)];
		const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
		for (let x = 0; x < stride; x++) {
			const left = x >= channels ? rows[y * stride + x - channels] : 0;
			const up = y > 0 ? rows[(y - 1) * stride + x] : 0;
			const corner = x >= channels && y > 0 ? rows[(y - 1) * stride + x - channels] : 0;
			let predictor = 0;
			if (filter === 1) {
				predictor = left;
			} else if (filter === 2) {
				predictor = up;
			} else if (filter === 3) {
				predictor = (left + up) >> 1;
			} else if (filter === 4) {
				const estimate = left + up - corner;
				const [a, b, c] = [Math.abs(estimate - left), Math.abs(estimate - up), Math.abs(estimate - corner)];
				predictor = a <= b && a <= c ? left : b <= c ? up : corner;
			}
			rows[y * stride + x] = (line[x] + predictor) & 0xff;
		}
	}
	if (channels === 4) {
		return { width, height, rgba: rows };
	}
	const rgba = Buffer.alloc(width * height * 4, 0xff);
	for (let pixel = 0; pixel < width * height; pixel++) {
		rows.copy(rgba, pixel * 4, pixel * 3, pixel * 3 + 3);
	}
	return { width, height, rgba };
}

/** A 24-bit, bottom-up BMP of `rgba` composited over white, as NSIS loads it. */
export function encodeBmp(width: number, height: number, rgba: Buffer): Buffer {
	const stride = Math.ceil(width * 3 / 4) * 4;
	const header = 54;
	const bmp = Buffer.alloc(header + stride * height);
	bmp.write('BM', 0, 'latin1');
	bmp.writeUInt32LE(bmp.length, 2);
	bmp.writeUInt32LE(header, 10);
	bmp.writeUInt32LE(40, 14);
	bmp.writeInt32LE(width, 18);
	bmp.writeInt32LE(height, 22);
	bmp.writeUInt16LE(1, 26);
	bmp.writeUInt16LE(24, 28);
	bmp.writeUInt32LE(stride * height, 34);
	for (let y = 0; y < height; y++) {
		const row = header + (height - 1 - y) * stride;
		for (let x = 0; x < width; x++) {
			const source = (y * width + x) * 4;
			const alpha = rgba[source + 3] / 255;
			const over = (channel: number) => Math.round(rgba[source + channel] * alpha + 255 * (1 - alpha));
			bmp[row + x * 3] = over(2);
			bmp[row + x * 3 + 1] = over(1);
			bmp[row + x * 3 + 2] = over(0);
		}
	}
	return bmp;
}

/** Renders every installer image into `assets/`. */
export async function renderInstallerAssets(root: string, signal: AbortSignal): Promise<void> {
	const source = await readFile(join(root, 'assets/tray/mark-black.svg'), 'utf8');
	const markPath = [...source.matchAll(/\sd="([^"]+)"/g)].map(match => match[1]).join(' ');
	if (!markPath) {
		throw new Error('assets/tray/mark-black.svg has no path');
	}
	const work = await mkdtemp(join(tmpdir(), 'vnilink-installer-art-'));
	try {
		for (const image of installerImages) {
			const svg = join(work, 'image.svg');
			await writeFile(svg, image.svg(markPath));
			for (const scale of image.scales) {
				const png = image.format === 'png' ? join(root, 'assets', installerImageFile(image, scale)) : join(work, 'image.png');
				try {
					await processOutput('rsvg-convert', ['-w', String(image.width * scale), '-h', String(image.height * scale), '-o', png, svg], { signal });
				} catch (error) {
					signal.throwIfAborted();
					throw new Error('Rendering installer artwork needs rsvg-convert (librsvg: `brew install librsvg` or `apt install librsvg2-bin`).', { cause: error });
				}
				if (image.format === 'bmp') {
					const { width, height, rgba } = decodePng(await readFile(png));
					await writeFile(join(root, 'assets', installerImageFile(image, scale)), encodeBmp(width, height, rgba));
				}
			}
		}
		console.log(`Installer artwork rendered: ${installerImages.map(image => image.path).join(', ')}`);
	} finally {
		await rm(work, { recursive: true, force: true });
	}
}
