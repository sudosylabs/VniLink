import type { DesktopPictureRole } from '../../services/remoteDesktop/remoteDesktopService.js';

/** A tile's side in picture pixels: what is redrawn when part of a still changes. */
const TILE = 32;

/**
 * The sample is half the picture's size, so the browser's downscale is an
 * exact average of each 2 × 2 block: a change one pixel wide, such as a text
 * caret, still moves its sample by half its contrast.
 */
const SAMPLE_SCALE = 2;

/** A sample value moving further than this, in any colour, is a change rather than the encoder refining. */
const CHANGED = 16;

/** The share of tiles that, once changed, makes the stream's picture replace the still everywhere. */
const MOST = 0.5;

/** A part of the picture, in its pixels. */
export interface PictureRect {
	readonly x: number;
	readonly y: number;
	readonly width: number;
	readonly height: number;
}

/** What to draw of a picture: all of it, or only these parts of it, over a still shown before. */
export type StillPlan = 'whole' | readonly PictureRect[];

interface Sample {
	readonly width: number;
	readonly height: number;
	readonly pixels: Uint8ClampedArray;
}

/**
 * Keeps a 4:4:4 still where the screen has not changed since it. The stream
 * goes on behind a still: its repeats of the same screen are not drawn over
 * it, and a changed picture is compared tile by tile with the stream's
 * picture of the screen the still shows, so a clock or a caret changing
 * redraws only its tiles. Once most of the screen changed, the stream
 * replaces the still everywhere, as it does when it cannot compare.
 */
export class StillCompositor {
	private showing = false;
	/** The stream's latest picture of the screen, sampled; taken from repeats, which change nothing. */
	private base: Sample | undefined;
	/** Tiles the stream has redrawn over the still, by row. */
	private changed = new Uint8Array(0);
	private readonly sampler = new OffscreenCanvas(1, 1);
	private readonly samplerContext = this.sampler.getContext('2d', { willReadFrequently: true });

	/** Whether a still shows, at least in part. */
	public get stillShown(): boolean {
		return this.showing;
	}

	/** What of `frame`, a picture in its `role`, to draw now. */
	public plan(frame: VideoFrame, role: DesktopPictureRole): StillPlan {
		const width = frame.displayWidth;
		const height = frame.displayHeight;
		if (role === 'still') {
			this.showing = true;
			this.changed = new Uint8Array(this.columns(width) * this.rows(height));
			return 'whole';
		}
		if (!this.showing) {
			// Only a repeat is a picture of a screen a still may show next.
			this.base = role === 'unchanged' ? this.sample(frame) : undefined;
			return 'whole';
		}
		const sample = this.sample(frame);
		const base = this.base;
		this.base = sample;
		if (!sample || this.changed.length !== this.columns(width) * this.rows(height)) {
			return this.release();
		}
		if (role === 'changed') {
			// A change with nothing of the stream to compare it with may be anywhere.
			if (base?.width !== sample.width || base.height !== sample.height) {
				return this.release();
			}
			this.mark(base, sample, width, height);
			if (this.changed.reduce((count, tile) => count + tile, 0) > this.changed.length * MOST) {
				return this.release();
			}
		}
		// A repeat redraws what the stream already shows, refined; a change also redraws its new tiles.
		return this.rects(width, height);
	}

	/** Forgets the still and what the stream showed, as a new session starts. */
	public reset(): void {
		this.showing = false;
		this.base = undefined;
		this.changed = new Uint8Array(0);
	}

	private release(): StillPlan {
		this.showing = false;
		this.changed = new Uint8Array(0);
		return 'whole';
	}

	private columns(width: number): number {
		return Math.ceil(width / TILE);
	}

	private rows(height: number): number {
		return Math.ceil(height / TILE);
	}

	/** The picture at half its size, or nothing where the browser cannot read it back. */
	private sample(frame: VideoFrame): Sample | undefined {
		const context = this.samplerContext;
		if (!context) {
			return undefined;
		}
		const width = Math.ceil(frame.displayWidth / SAMPLE_SCALE);
		const height = Math.ceil(frame.displayHeight / SAMPLE_SCALE);
		if (this.sampler.width !== width || this.sampler.height !== height) {
			this.sampler.width = width;
			this.sampler.height = height;
		}
		try {
			context.drawImage(frame, 0, 0, width, height);
			return { width, height, pixels: context.getImageData(0, 0, width, height).data };
		} catch {
			return undefined;
		}
	}

	/** Marks the tiles where `next` moved past {@link CHANGED} from `base`. */
	private mark(base: Sample, next: Sample, width: number, height: number): void {
		const columns = this.columns(width);
		const side = TILE / SAMPLE_SCALE;
		for (let row = 0; row < this.rows(height); row++) {
			for (let column = 0; column < columns; column++) {
				const index = row * columns + column;
				if (this.changed[index]) {
					continue;
				}
				const bottom = Math.min(next.height, (row + 1) * side);
				const right = Math.min(next.width, (column + 1) * side);
				tile: for (let y = row * side; y < bottom; y++) {
					for (let at = (y * next.width + column * side) * 4, end = (y * next.width + right) * 4; at < end; at++) {
						if ((at & 3) !== 3 && Math.abs(next.pixels[at] - base.pixels[at]) > CHANGED) {
							this.changed[index] = 1;
							break tile;
						}
					}
				}
			}
		}
	}

	/** The changed tiles as rectangles, one per run along a row. */
	private rects(width: number, height: number): PictureRect[] {
		const columns = this.columns(width);
		const rects: PictureRect[] = [];
		for (let row = 0; row < this.rows(height); row++) {
			let column = 0;
			while (column < columns) {
				if (!this.changed[row * columns + column]) {
					column++;
					continue;
				}
				const start = column;
				while (column < columns && this.changed[row * columns + column]) {
					column++;
				}
				const x = start * TILE;
				const y = row * TILE;
				rects.push({ x, y, width: Math.min(width, column * TILE) - x, height: Math.min(height, y + TILE) - y });
			}
		}
		return rects;
	}
}
