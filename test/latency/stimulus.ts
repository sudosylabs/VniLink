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

/** A self-contained page drawing the strip at `cell` CSS pixels a cell. */
export function stimulusPage(cell: number): string {
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
canvas.width = Math.round(innerWidth * scale);
canvas.height = Math.round(innerHeight * scale);
function draw(time) {
	const strip = cells(Math.round(time));
	context.fillStyle = '#fff';
	context.fillRect(0, 0, canvas.width, canvas.height);
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
	requestAnimationFrame(draw);
}
requestAnimationFrame(draw);`;
	return `<!doctype html><meta charset="utf-8"><title>Latency stimulus</title>
<style>html,body{margin:0;height:100%;background:#fff;overflow:hidden}canvas{display:block;width:100%;height:100%}</style>
<canvas></canvas><script>${script}</script>`;
}
