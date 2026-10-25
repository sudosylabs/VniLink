/**
 * Perceived quality of a remote desktop application, measured from the
 * screen of the computer viewing with it (`npm run measure:perceived`).
 *
 * The page in `test/perceived/stimulus.ts` runs full screen on the computer
 * being controlled, started here over SSH. Someone opens the application
 * under test on this computer, full screen, showing that computer; VniLink
 * and RustDesk are measured the same way, as black boxes. The driver
 * (`crates/vnilink-macos/examples/perceived.rs`) reads the page back from
 * this screen and moves and clicks the real pointer:
 * - the methods are established ones: timecodes drawn on screen and read
 *   back from a capture (as vDelay measures video applications), synthetic
 *   input timed to the first changed picture (as AMD's Frame Latency Meter
 *   measures click latency), and FFmpeg's SSIM against the exact reference;
 * - times on this side are this computer's: the display's for pictures and
 *   the driver's for input, one clock, no synchronisation;
 * - it does not cover the display panel's own latency, which a photodiode
 *   tester measures.
 *
 * The viewer side runs on macOS; the controlled computer runs Linux.
 */
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { perceivedCard, perceivedLayout } from '../../test/perceived/stimulus.ts';
import { ProcessRunner } from './processes.ts';
import { loadProduct } from './product.ts';
import { remoteShell, shellWord, type SshTarget } from './remote.ts';

export interface PerceivedOptions {
	/** The computer being controlled, and its built checkout. */
	readonly remote: SshTarget & { readonly root: string };
	/** The application under test, as the summary names it. */
	readonly label: string;
	/** How long the driver waits for the page to appear on this screen. */
	readonly waitSeconds: number;
	readonly output: string;
}

export function perceivedOptions(root: string, args: readonly string[]): PerceivedOptions {
	const { values } = parseArgs({
		args: [...args],
		options: {
			remote: { type: 'string' }, 'remote-root': { type: 'string' }, 'ssh-config': { type: 'string' },
			label: { type: 'string' }, wait: { type: 'string', default: '300' }, output: { type: 'string' },
		},
		strict: true,
	});
	const remoteRoot = values['remote-root'];
	if (values.remote === undefined || !remoteRoot?.startsWith('/')) {
		throw new Error('--remote names the computer being controlled and --remote-root its built checkout, an absolute path');
	}
	const label = values.label?.trim();
	if (!label || !/^[\w .-]{1,40}$/.test(label)) {
		throw new Error('--label names the application under test (letters, digits, space, dot, dash)');
	}
	const waitSeconds = Number(values.wait);
	if (!Number.isInteger(waitSeconds) || waitSeconds < 10 || waitSeconds > 1800) {
		throw new Error('--wait must be 10..1800 seconds');
	}
	const stamp = new Date().toISOString().replaceAll(':', '-');
	return {
		remote: { destination: values.remote, root: remoteRoot, sshConfig: values['ssh-config'] === undefined ? undefined : resolve(values['ssh-config']) },
		label,
		waitSeconds,
		output: resolve(values.output ?? join(root, '.build/perceived', `${label.replaceAll(/\W+/g, '-')}-${stamp}`)),
	};
}

/** p50 and p95 of a sample, and how many values it holds. */
export interface Spread {
	readonly p50: number;
	readonly p95: number;
	readonly max: number;
	readonly count: number;
}

function spread(values: readonly number[]): Spread | undefined {
	const sorted = [...values].sort((a, b) => a - b);
	const at = (fraction: number): number => sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(fraction * sorted.length) - 1))];
	return sorted.length ? { p50: at(0.5), p95: at(0.95), max: sorted[sorted.length - 1], count: sorted.length } : undefined;
}

export interface PerceivedSummary {
	readonly label: string;
	/** Viewer pixels per host pixel: above 1 the picture is enlarged. */
	readonly scale: number;
	readonly host: { readonly width: number; readonly height: number };
	readonly viewer: { readonly width: number; readonly height: number };
	/** How often the picture moved and the measurement started again. */
	readonly restarts: number;
	/** From a pointer move to the first picture showing the host's pointer there, ms. */
	readonly pointerJump: Spread | undefined;
	/** Moves never seen arriving within their window. */
	readonly pointerMissed: number;
	/** Where the host's pointer settled against where it was sent, host pixels. */
	readonly pointerError: Spread | undefined;
	/** Along a continuous circle, how old the shown pointer position was, ms. */
	readonly pointerTrail: Spread | undefined;
	/** From a click to the first picture showing it counted, ms. */
	readonly click: Spread | undefined;
	readonly clicksMissed: number;
	/** Pictures this screen showed a second while the band moved. */
	readonly framesPerSecond: number | undefined;
	/** Time between consecutive pictures, ms. */
	readonly frameInterval: Spread | undefined;
	/** Gaps over 50 ms: a visible stutter. */
	readonly stutters: number;
	/** The share of the host's frames this screen showed. */
	readonly hostFramesShown: number | undefined;
	/** SSIM of the test card against the reference at each time after a swap, median over swaps. */
	readonly sharpness: readonly { readonly afterMs: number; readonly ssim: number }[];
	/** SSIM once settled, and when the card first came within 0.01 of it, ms after a swap. */
	readonly settledSsim: number | undefined;
	readonly sharpAfterMs: number | undefined;
}

interface Event {
	readonly type: string;
	readonly [field: string]: unknown;
}

const number = (value: unknown): number | undefined => typeof value === 'number' && Number.isFinite(value) ? value : undefined;

/** The driver's events and each crop's SSIM (by file) as one summary. */
export function summarisePerceived(label: string, lines: string, ssim: ReadonlyMap<string, number>): PerceivedSummary {
	const all = lines.split('\n').filter(line => line.trim()).map(line => JSON.parse(line) as Event);
	// The picture moving restarts the measurement: only the last run counts.
	const last = all.findLastIndex(event => event.type === 'located');
	if (last < 0) {
		throw new Error('The driver never found the page on this screen');
	}
	const events = all.slice(last);
	const located = events[0];
	const frames = events
		.filter(event => event.type === 'frame' && number(event.t) !== undefined)
		.map(event => ({ t: number(event.t) ?? 0, frame: number(event.frame), clicks: number(event.clicks), x: number(event.x), y: number(event.y) }));
	type Frame = (typeof frames)[number];
	const after = (t: number, until: number, ok: (frame: Frame) => boolean): Frame | undefined =>
		frames.find(frame => frame.t > t && frame.t <= until && ok(frame));
	const moves = events.filter(event => event.type === 'move');
	const jumps = moves.filter(event => event.kind === 'jump');
	const path = moves.filter(event => event.kind === 'path');
	const clicks = events.filter(event => event.type === 'click');
	const near = (frame: Frame, x: number, y: number, tolerance: number): boolean =>
		frame.x !== undefined && frame.y !== undefined && Math.abs(frame.x - x) <= tolerance && Math.abs(frame.y - y) <= tolerance;
	const jumpLatency: number[] = [];
	const jumpError: number[] = [];
	let pointerMissed = 0;
	jumps.forEach((jump, index) => {
		const t = number(jump.t) ?? 0;
		const until = number(jumps[index + 1]?.t) ?? number(path[0]?.t) ?? Number.POSITIVE_INFINITY;
		const [x, y] = [number(jump.hostX) ?? 0, number(jump.hostY) ?? 0];
		const hit = after(t, until, frame => near(frame, x, y, 3));
		if (hit) {
			jumpLatency.push(hit.t - t);
		} else {
			pointerMissed++;
		}
		const settled = frames.filter(frame => frame.t > t && frame.t <= until && frame.x !== undefined).at(-1);
		if (settled?.x !== undefined && settled.y !== undefined) {
			jumpError.push(Math.hypot(settled.x - x, settled.y - y));
		}
	});
	// Each picture during the circle against the move it shows.
	const trail: number[] = [];
	if (path.length) {
		const first = number(path[0].t) ?? 0;
		const last = (number(path.at(-1)?.t) ?? first) + 500;
		for (const frame of frames.filter(frame => frame.t > first && frame.t <= last)) {
			// The circle comes round once a second: only the last half second's moves.
			const shown = path.findLast(move => {
				const t = number(move.t) ?? 0;
				return t < frame.t && t > frame.t - 500 && near(frame, number(move.hostX) ?? 0, number(move.hostY) ?? 0, 2);
			});
			if (shown) {
				trail.push(frame.t - (number(shown.t) ?? 0));
			}
		}
	}
	const clickLatency: number[] = [];
	let clicksMissed = 0;
	clicks.forEach((click, index) => {
		const t = number(click.t) ?? 0;
		const until = number(clicks[index + 1]?.t) ?? t + 1000;
		const before = frames.filter(frame => frame.t <= t && frame.clicks !== undefined).at(-1)?.clicks ?? 0;
		const hit = after(t, until, frame => frame.clicks !== undefined && frame.clicks > before);
		if (hit) {
			clickLatency.push(hit.t - t);
		} else {
			clicksMissed++;
		}
	});
	// Pacing: the pictures after the clicks, the pointer resting.
	const settledFrom = (number(clicks.at(-1)?.t) ?? 0) + 500;
	const pacing = frames.filter(frame => frame.t > settledFrom);
	const intervals = pacing.slice(1).map((frame, index) => frame.t - pacing[index].t);
	const counters = pacing.map(frame => frame.frame).filter((value): value is number => value !== undefined);
	const span = pacing.length > 1 ? (pacing[pacing.length - 1].t - pacing[0].t) / 1000 : 0;
	const hostFrames = counters.length > 1 ? counters[counters.length - 1] - counters[0] + 1 : 0;
	// Sharpness after each swap, by the scheduled time it was asked at.
	const crops = events.filter(event => event.type === 'crop' && typeof event.file === 'string' && ssim.has(event.file));
	const buckets = new Map<number, number[]>();
	const scheduled = [0, 100, 250, 500, 750, 1000, 1500, 2000, 3000, 4000, 6000, 7500];
	for (const crop of crops) {
		const since = number(crop.sinceMs) ?? 0;
		const slot = scheduled.findLast(at => at <= since + 50) ?? 0;
		const values = buckets.get(slot) ?? [];
		values.push(ssim.get(crop.file as string) ?? 0);
		buckets.set(slot, values);
	}
	const median = (values: readonly number[]): number => spread(values)?.p50 ?? 0;
	const sharpness = [...buckets].sort(([a], [b]) => a - b).map(([afterMs, values]) => ({ afterMs, ssim: median(values) }));
	const settled = sharpness.filter(point => point.afterMs >= 6000).map(point => point.ssim);
	const settledSsim = settled.length ? median(settled) : undefined;
	return {
		label,
		scale: number(located.scaleX) ?? 0,
		host: { width: number(located.hostWidth) ?? 0, height: number(located.hostHeight) ?? 0 },
		viewer: { width: number(located.viewerWidth) ?? 0, height: number(located.viewerHeight) ?? 0 },
		restarts: all.filter(event => event.type === 'moved').length,
		pointerJump: spread(jumpLatency),
		pointerMissed,
		pointerError: spread(jumpError),
		pointerTrail: spread(trail),
		click: spread(clickLatency),
		clicksMissed,
		framesPerSecond: span > 0 ? (pacing.length - 1) / span : undefined,
		frameInterval: spread(intervals),
		stutters: intervals.filter(interval => interval > 50).length,
		hostFramesShown: hostFrames > 0 ? new Set(counters).size / hostFrames : undefined,
		sharpness,
		settledSsim,
		sharpAfterMs: settledSsim === undefined ? undefined : sharpness.find(point => point.ssim >= settledSsim - 0.01)?.afterMs,
	};
}

const ms = (spread: Spread | undefined): string => spread ? `${spread.p50.toFixed(0)} / ${spread.p95.toFixed(0)}` : '-';

/** Several runs side by side, one column each. */
export function renderPerceived(runs: readonly PerceivedSummary[]): string {
	const row = (name: string, cell: (run: PerceivedSummary) => string): string => `| ${name} | ${runs.map(cell).join(' | ')} |`;
	const fixed = (value: number | undefined, digits: number): string => value === undefined ? '-' : value.toFixed(digits);
	return [
		'# Perceived quality',
		'',
		`| | ${runs.map(run => run.label).join(' | ')} |`,
		`|---|${runs.map(() => '---').join('|')}|`,
		row('Picture scale (viewer px per host px)', run => `${run.scale.toFixed(2)} (${String(run.host.width)}x${String(run.host.height)} on ${String(run.viewer.width)}x${String(run.viewer.height)})`),
		row('Picture moved, measurement restarted', run => String(run.restarts)),
		row('Pointer jump, p50 / p95 ms', run => `${ms(run.pointerJump)}${run.pointerMissed ? ` (${String(run.pointerMissed)} missed)` : ''}`),
		row('Pointer error, p50 / max host px', run => run.pointerError ? `${run.pointerError.p50.toFixed(1)} / ${run.pointerError.max.toFixed(1)}` : '-'),
		row('Pointer trail in motion, p50 / p95 ms', run => ms(run.pointerTrail)),
		row('Click to picture, p50 / p95 ms', run => `${ms(run.click)}${run.clicksMissed ? ` (${String(run.clicksMissed)} missed)` : ''}`),
		row('Pictures a second', run => fixed(run.framesPerSecond, 1)),
		row('Picture interval, p50 / p95 ms', run => ms(run.frameInterval)),
		row('Stutters (gaps over 50 ms)', run => String(run.stutters)),
		row('Host frames shown', run => run.hostFramesShown === undefined ? '-' : `${(run.hostFramesShown * 100).toFixed(0)}%`),
		row('Card SSIM settled', run => fixed(run.settledSsim, 4)),
		row('Card sharp after a swap, ms', run => run.sharpAfterMs === undefined ? '-' : String(run.sharpAfterMs)),
		row('Card SSIM at 0 / 250 / 1000 / 2000 ms', run => [0, 250, 1000, 2000].map(at => fixed(run.sharpness.find(point => point.afterMs === at)?.ssim, 3)).join(' / ')),
		'',
	].join('\n');
}

/** The test card's variant as RGB at the host's size: the reference. */
export function cardReference(variant: number, width: number, height: number): Buffer {
	const pixels = Buffer.alloc(width * height * 3);
	for (const rect of perceivedCard(variant, width, height)) {
		for (let y = rect.y; y < Math.min(height, rect.y + rect.height); y++) {
			for (let x = rect.x; x < Math.min(width, rect.x + rect.width); x++) {
				pixels.set(rect.rgb, (y * width + x) * 3);
			}
		}
	}
	return pixels;
}

const run = promisify(execFile);

/** SSIM of a BGRA crop against the reference scaled to it with Lanczos, by FFmpeg. */
interface Size {
	readonly width: number;
	readonly height: number;
}

async function ssimOf(reference: string, size: Size, crop: string, cropSize: Size, signal: AbortSignal): Promise<number> {
	const { stderr } = await run('ffmpeg', [
		'-hide_banner', '-nostats',
		'-f', 'rawvideo', '-pix_fmt', 'rgb24', '-s', `${String(size.width)}x${String(size.height)}`, '-i', reference,
		'-f', 'rawvideo', '-pix_fmt', 'bgra', '-s', `${String(cropSize.width)}x${String(cropSize.height)}`, '-i', crop,
		'-lavfi', `[0:v]scale=${String(cropSize.width)}:${String(cropSize.height)}:flags=lanczos,format=gbrp[r];[1:v]format=gbrp[c];[r][c]ssim`,
		'-f', 'null', '-',
	], { signal, maxBuffer: 16 * 1024 * 1024 });
	const all = /All:([\d.]+)/.exec(stderr);
	if (!all) {
		throw new Error(`FFmpeg gave no SSIM for ${crop}`);
	}
	return Number(all[1]);
}

/** The main display's width in pixels, from the system's own report. */
async function displayPixels(signal: AbortSignal): Promise<number | undefined> {
	const { stdout } = await run('system_profiler', ['SPDisplaysDataType', '-json'], { signal, maxBuffer: 16 * 1024 * 1024 });
	const report = JSON.parse(stdout) as { SPDisplaysDataType?: { spdisplays_ndrvs?: { spdisplays_main?: string; _spdisplays_pixels?: string }[] }[] };
	const main = report.SPDisplaysDataType?.flatMap(card => card.spdisplays_ndrvs ?? []).find(display => display.spdisplays_main === 'spdisplays_yes');
	const width = /^(\d+) x \d+$/.exec(main?._spdisplays_pixels ?? '')?.[1];
	return width === undefined ? undefined : Number(width);
}

/** SSIM of every crop in `directory`, then the summary there and on the console. */
export async function analysePerceived(directory: string, label: string, signal: AbortSignal): Promise<PerceivedSummary> {
	const lines = await readFile(join(directory, 'events.jsonl'), 'utf8');
	const events = lines.split('\n').filter(line => line.trim()).map(line => JSON.parse(line) as Event);
	const located = events.find(event => event.type === 'located');
	const cell = number(located?.cell) ?? 12;
	const layout = perceivedLayout(number(located?.hostWidth) ?? 0, number(located?.hostHeight) ?? 0, cell / 12);
	const ssim = new Map<string, number>();
	for (const variant of [0, 1]) {
		await writeFile(join(directory, `card-${String(variant)}.rgb`), cardReference(variant, layout.card.width, layout.card.height));
	}
	for (const crop of events.filter(event => event.type === 'crop')) {
		const file = crop.file as string;
		const card = number(crop.card) ?? 0;
		const size = { width: number(crop.width) ?? 0, height: number(crop.height) ?? 0 };
		ssim.set(file, await ssimOf(join(directory, `card-${String(card % 2)}.rgb`), layout.card, join(directory, file), size, signal));
	}
	const summary = summarisePerceived(label, lines, ssim);
	await writeFile(join(directory, 'summary.json'), JSON.stringify(summary, null, 2));
	const text = renderPerceived([summary]);
	await writeFile(join(directory, 'summary.md'), text);
	console.log(text);
	console.log(`Evidence: ${directory}`);
	return summary;
}

export async function measurePerceived(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	if (process.platform !== 'darwin') {
		throw new Error('measure:perceived views from macOS');
	}
	const options = perceivedOptions(root, args);
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vp-'));
	const shell = remoteShell(options.remote, work, signal);
	const unit = `vnilink-perceived-${String(process.pid)}`;
	try {
		await mkdir(options.output, { recursive: true });
		await processes.run('cargo', ['build', '--release', '--locked', '-p', 'vnilink-macos', '--example', 'perceived'], { cwd: root });
		const pixels = await displayPixels(signal);
		const electron = `${options.remote.root}/.build/electron/${(await loadProduct(root)).applicationName}`;
		await shell.output(`test -x ${shellWord(electron)} && test -f ${shellWord(`${options.remote.root}/out/perceived.cjs`)}`).catch(() => {
			throw new Error(`${options.remote.destination}:${options.remote.root} has no built stimulus: run npm run build there first`);
		});
		await shell.output([
			'systemd-run', '--user', '--collect', '--quiet', `--unit=${unit}`, `--working-directory=${options.remote.root}`,
			electron, '--ozone-platform=x11', `${options.remote.root}/out/perceived.cjs`,
		].map(shellWord).join(' '));
		console.log(`The stimulus is maximised on ${options.remote.destination}. Open ${options.label} on this Mac to it now, full screen, showing its whole screen; measuring starts when the page is found (up to ${String(options.waitSeconds)} s), then the pointer moves by itself for about 45 s.`);
		await processes.run(join(root, 'target/release/examples/perceived'), [options.output, String(options.waitSeconds), ...(pixels ? [String(pixels)] : [])]);
		await analysePerceived(options.output, options.label, signal);
	} finally {
		await shell.output(`systemctl --user stop ${shellWord(unit)} 2>/dev/null; true`).catch(() => undefined);
		await shell.close();
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}

/** Runs measured before, side by side: `--compare <dir> <dir>...`. */
export async function comparePerceived(directories: readonly string[]): Promise<void> {
	const runs = await Promise.all(directories.map(async directory => JSON.parse(await readFile(join(directory, 'summary.json'), 'utf8')) as PerceivedSummary));
	console.log(renderPerceived(runs));
}
