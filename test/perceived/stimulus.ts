/**
 * The perceived-quality stimulus (`npm run measure:perceived`): a page shown
 * maximised on the computer being controlled, through whichever remote
 * desktop application is under test. Everything it shows is read back from
 * the viewing computer's own screen, so the application is a black box.
 *
 * In device pixels, with `S` the strip cell:
 * - a column of timecode strips at the top left, read by
 *   `vnilink_media::measure::timecode`: an anchor, the frame counter, the
 *   click count, the pointer's x and y, the test card's generation, and the
 *   page's width and height;
 * - a second anchor at the bottom right; the two anchors give the mapping
 *   between viewer and host pixels;
 * - a band below the column that moves on every frame, an encoding load;
 * - a test card of rectangles, two variants that swap every
 *   {@link CARD_PERIOD_MS}, drawn pixel for pixel so the task can rasterise
 *   the same rectangles as the reference.
 *
 * A right click freezes the page, so the last phase measures a screen that
 * has stopped changing, where an application may refine or replace its
 * picture.
 *
 * `crates/vnilink-macos/examples/perceived.rs` keeps a copy of
 * {@link perceivedLayout}; `test/tooling/perceived.test.mjs` keeps the two
 * alike.
 */

/** Strip values that mark the two anchors. */
export const ANCHOR_TOP_LEFT = 0xa5a5a;

export const ANCHOR_BOTTOM_RIGHT = 0x5a5a5;

/** How long each test card variant stays before the other replaces it. */
export const CARD_PERIOD_MS = 8000;

/** Strips in the top-left column, in order from the top. */
export const COLUMN = ['anchor', 'frame', 'clicks', 'pointerX', 'pointerY', 'card', 'width', 'height'] as const;

export interface Rect {
	readonly x: number;
	readonly y: number;
	readonly width: number;
	readonly height: number;
}

export interface PerceivedLayout {
	/** Strip cell, device pixels. */
	readonly cell: number;
	/** Each column strip, left edge of its quiet zone and top. */
	readonly column: readonly Rect[];
	readonly anchorBottomRight: Rect;
	readonly band: Rect;
	readonly card: Rect;
}

/**
 * Where everything is on a page of `width` by `height` device pixels with
 * `ratio` device pixels per CSS pixel. Self-contained: the page carries this
 * function's own source.
 */
export function perceivedLayout(width: number, height: number, ratio: number): PerceivedLayout {
	const cell = Math.max(8, Math.round(12 * ratio));
	const margin = 2 * cell;
	// 46 cells and a quiet zone of two either side.
	const stripWidth = 50 * cell;
	const stripHeight = 3 * cell;
	const pitch = stripHeight + 2 * cell;
	const column = [];
	for (let index = 0; index < 8; index++) {
		column.push({ x: margin, y: margin + index * pitch, width: stripWidth, height: stripHeight });
	}
	const bottom = height - margin - stripHeight;
	const bandTop = margin + 8 * pitch;
	const cardLeft = margin + stripWidth + 4 * cell;
	const cardWidth = Math.max(64, Math.floor(Math.min(1024, width - cardLeft - margin) / 8) * 8);
	const cardHeight = Math.max(64, Math.floor(Math.min(768, bottom - 2 * cell - margin) / 8) * 8);
	return {
		cell,
		column,
		anchorBottomRight: { x: width - margin - stripWidth, y: bottom, width: stripWidth, height: stripHeight },
		band: { x: margin, y: bandTop, width: stripWidth, height: Math.max(cell, bottom - 2 * cell - bandTop) },
		card: { x: cardLeft, y: margin, width: cardWidth, height: cardHeight },
	};
}

/** One filled rectangle of the test card, in card pixels, colour as RGB. */
export interface CardRect extends Rect {
	readonly rgb: readonly [number, number, number];
}

/**
 * The test card's rectangles for variant 0 or 1 at `width` by `height`.
 * What a remote picture loses shows here first: one- and two-pixel lines
 * and checks, small text-like strokes in saturated colours on white and in
 * white on dark, and a colour chart. Variant 1 swaps the background and the
 * arrangement, so a swap changes the whole card. Deterministic and
 * self-contained: the page carries this function's own source, and the task
 * rasterises the same list as the reference.
 */
export function perceivedCard(variant: number, width: number, height: number): CardRect[] {
	const rects: CardRect[] = [];
	const dark = variant === 1;
	const paper: [number, number, number] = dark ? [24, 28, 48] : [255, 255, 255];
	const ink: [number, number, number] = dark ? [255, 255, 255] : [0, 0, 0];
	const colours: [number, number, number][] = [[200, 30, 30], [30, 60, 210], [240, 140, 0], [30, 140, 60], [140, 40, 160], [0, 140, 150]];
	rects.push({ x: 0, y: 0, width, height, rgb: paper });
	let seed = 1 + variant * 7919;
	const next = (limit: number): number => {
		seed = (seed * 1103515245 + 12345) % 2147483648;
		return (seed >>> 8) % limit;
	};
	const half = Math.floor(width / 2);
	// Line pairs and checks: one, two and three pixels.
	const patterns = (left: number, top: number): void => {
		for (let size = 1; size <= 3; size++) {
			const x0 = left + (size - 1) * 72;
			for (let x = 0; x < 64; x += 2 * size) {
				rects.push({ x: x0 + x, y: top, width: size, height: 64, rgb: ink });
			}
			for (let y = 0; y < 64; y += 2 * size) {
				rects.push({ x: x0, y: top + 72 + y, width: 64, height: size, rgb: ink });
			}
			for (let y = 0; y < 64; y += size) {
				for (let x = (y / size) % 2 === 0 ? 0 : size; x < 64; x += 2 * size) {
					rects.push({ x: x0 + x, y: top + 144 + y, width: size, height: size, rgb: ink });
				}
			}
		}
	};
	// Text-like strokes: glyphs five wide and seven high, at one and two
	// pixels a stroke, in each colour.
	const words = (left: number, top: number, right: number, bottom: number): void => {
		let y = top;
		for (let row = 0; y + 14 <= bottom; row++) {
			const scale = row % 3 === 2 ? 2 : 1;
			const colour = row % 4 === 3 ? ink : colours[row % colours.length];
			let x = left;
			while (x + 6 * scale <= right) {
				const letters = 2 + next(6);
				for (let letter = 0; letter < letters && x + 6 * scale <= right; letter++) {
					// A stem, a bar and a bowl or a tail, as letters are built.
					rects.push({ x: x + next(2) * 4 * scale, y, width: scale, height: 7 * scale, rgb: colour });
					rects.push({ x, y: y + next(3) * 3 * scale, width: 5 * scale, height: scale, rgb: colour });
					rects.push({
						x: x + next(4) * scale, y: y + (2 + next(4)) * scale,
						width: (1 + next(3)) * scale, height: (1 + next(2)) * scale, rgb: colour,
					});
					x += 6 * scale;
				}
				x += 4 * scale;
			}
			y += 10 * scale;
		}
	};
	const chart = (left: number, top: number): void => {
		const swatches: [number, number, number][] = [
			...colours, [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255], [255, 0, 255],
			[128, 128, 128], [64, 64, 64], [192, 192, 192], [255, 200, 160], [160, 80, 40], [90, 120, 60],
			[70, 110, 160], [230, 190, 40], [150, 30, 70], [20, 20, 20],
		];
		swatches.forEach((rgb, index) => {
			rects.push({ x: left + (index % 6) * 40, y: top + Math.floor(index / 6) * 32, width: 36, height: 28, rgb });
		});
	};
	const left = dark ? half + 16 : 16;
	const other = dark ? 16 : half + 16;
	patterns(left, 16);
	chart(left, 240);
	words(other, 16, other + half - 32, height - 16);
	words(left, 384, left + half - 32, height - 16);
	return rects;
}

/** The cells of a strip for `value`, dark = `true`, as `vnilink_media::measure::timecode` reads them. */
export function perceivedCells(value: number): boolean[] {
	const masked = value & 0xfffff;
	const bits: boolean[] = [];
	for (let bit = 19; bit >= 0; bit--) {
		bits.push(((masked >> bit) & 1) === 1);
	}
	return [true, false, true, false, ...bits, ...bits.map(set => !set), true, true];
}

/** The page, self-contained, carrying the three functions above as source. */
export function perceivedPage(): string {
	const script = `
${perceivedLayout.toString()}
${perceivedCard.toString()}
${perceivedCells.toString()}
const ANCHOR_TOP_LEFT = ${String(ANCHOR_TOP_LEFT)};
const ANCHOR_BOTTOM_RIGHT = ${String(ANCHOR_BOTTOM_RIGHT)};
const CARD_PERIOD_MS = ${String(CARD_PERIOD_MS)};
const canvas = document.querySelector('canvas');
const context = canvas.getContext('2d', { alpha: false });
let ratio = 1;
let layout;
const values = { frame: 0, clicks: 0, pointerX: 0, pointerY: 0, card: 0 };
const order = ['anchor', 'frame', 'clicks', 'pointerX', 'pointerY', 'card', 'width', 'height'];
function strip(rect, value) {
	context.fillStyle = '#fff';
	context.fillRect(rect.x, rect.y, rect.width, rect.height);
	context.fillStyle = '#000';
	perceivedCells(value).forEach((darkCell, index) => {
		if (darkCell) {
			context.fillRect(rect.x + (2 + index) * layout.cell, rect.y, layout.cell, rect.height);
		}
	});
}
function card(variant) {
	const { x, y, width, height } = layout.card;
	for (const rect of perceivedCard(variant, width, height)) {
		context.fillStyle = 'rgb(' + rect.rgb.join(',') + ')';
		context.fillRect(x + rect.x, y + rect.y, rect.width, rect.height);
	}
}
// Lays the page out for the window's size, again whenever it changes: the
// window is maximised, which a desktop may finish after the first frame.
function place() {
	ratio = window.devicePixelRatio;
	canvas.width = Math.round(innerWidth * ratio);
	canvas.height = Math.round(innerHeight * ratio);
	layout = perceivedLayout(canvas.width, canvas.height, ratio);
	context.fillStyle = '#fff';
	context.fillRect(0, 0, canvas.width, canvas.height);
	strip(layout.anchorBottomRight, ANCHOR_BOTTOM_RIGHT);
	card(values.card % 2);
}
place();
addEventListener('resize', place);
const started = performance.now();
addEventListener('pointermove', event => {
	values.pointerX = Math.round(event.clientX * ratio);
	values.pointerY = Math.round(event.clientY * ratio);
});
// A right click freezes the page for good: nothing on it changes again, so
// a still screen's picture can be measured.
let frozen = false;
addEventListener('pointerdown', event => {
	if (event.button === 2) {
		frozen = true;
	} else {
		values.clicks++;
	}
});
addEventListener('contextmenu', event => {
	event.preventDefault();
});
let phase = 0;
function draw(time) {
	if (frozen) {
		return;
	}
	values.frame++;
	const generation = Math.floor((time - started) / CARD_PERIOD_MS);
	if (generation !== values.card) {
		values.card = generation;
		card(generation % 2);
	}
	const shown = { ...values, anchor: ANCHOR_TOP_LEFT, width: canvas.width, height: canvas.height };
	layout.column.forEach((rect, index) => strip(rect, shown[order[index]]));
	// The band: stripes that move a few pixels a frame and a block that
	// crosses it, as scrolling and a moving window load an encoder.
	const band = layout.band;
	phase = (phase + 3) % 48;
	for (let y = 0; y < band.height; y += 8) {
		const shade = ((Math.floor((y + phase) / 24)) % 2) === 0 ? 235 : 60;
		context.fillStyle = 'rgb(' + shade + ',' + (255 - shade) + ',' + ((shade * 3) % 256) + ')';
		context.fillRect(band.x, band.y + y, band.width, Math.min(8, band.height - y));
	}
	const block = Math.round(band.height / 3);
	context.fillStyle = '#c62828';
	context.fillRect(band.x + ((values.frame * 4) % Math.max(1, band.width - block)), band.y + block, block, block);
	requestAnimationFrame(draw);
}
requestAnimationFrame(draw);`;
	return `<!doctype html><meta charset="utf-8"><title>Perceived stimulus</title>
<style>html,body{margin:0;height:100%;background:#fff;overflow:hidden;cursor:none}canvas{display:block;width:100%;height:100%}</style>
<canvas></canvas><script>${script}</script>`;
}
