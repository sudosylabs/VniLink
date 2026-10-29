/**
 * Perceived quality, measured from this Mac's screen
 * (`npm run measure:perceived`).
 *
 * By default everything runs on this Mac: the page in
 * `test/perceived/stimulus.ts` opens maximised here, and the run measures
 * this computer's own floor (synthetic input, the browser, the display and
 * capture), the baseline every remote desktop application adds to. With
 * `--peer`, the page runs on a second computer, started by the test agent
 * there (`node build/run.ts testkit:agent`) from its own checkout, and
 * someone opens the application under test on this Mac, full screen,
 * showing that computer; VniLink and RustDesk are measured the same way, as
 * black boxes. In both, the driver
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
 * The viewer side runs on macOS; the controlled computer may run any OS
 * the agent runs on.
 */
import { execFile } from 'node:child_process';
import { access, mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { perceivedCard, perceivedLayout } from '../../test/perceived/stimulus.ts';
import { assertBundleFresh } from './bundles.ts';
import { electronExecutable } from './electron.ts';
import { isLinux } from './platform.ts';
import { type LaunchedProcess, ProcessRunner } from './processes.ts';
import { agentPeer, openAgentSession, type AgentPeer } from './remote.ts';

export interface PerceivedOptions {
	/** The test agent on the computer being controlled; undefined runs the page on this Mac. */
	readonly peer: AgentPeer | undefined;
	/** What is measured, as the summary names it: the application under test, or this Mac's floor. */
	readonly label: string;
	/** How long the driver waits for the page to appear on this screen. */
	readonly waitSeconds: number;
	readonly output: string;
}

/** The label of a run on this Mac alone. */
export const LOCAL_LABEL = 'This Mac';

export const perceivedUsage = `npm run measure:perceived -- [options]
npm run measure:perceived -- --compare <directory> <directory>...

By default the page runs on this Mac and the run measures this computer's own
floor: synthetic input, the browser, the display and capture. Build first
(npm run build).

  --label <name>          what the summary calls the run (default: ${LOCAL_LABEL})
  --wait <seconds>        how long to wait for the page on this screen, 10..1800 (default: 300)
  --output <directory>    where the evidence goes (default: .build/perceived/<label>-<time>)

Optional, a second computer: the page runs there, started by the test agent
(node build/run.ts testkit:agent, built from the same commit), and the
application under test shows it here.

  --peer <file.vnl>       the agent's pairing file
  --peer-check <digits>   the digits the agent printed beside it
  --label <name>          required with --peer: the application under test`;

export function perceivedOptions(root: string, args: readonly string[]): PerceivedOptions {
	const { values } = parseArgs({
		args: [...args],
		options: {
			peer: { type: 'string' }, 'peer-check': { type: 'string' },
			label: { type: 'string' }, wait: { type: 'string', default: '300' }, output: { type: 'string' },
		},
		strict: true,
	});
	const peer = agentPeer(values.peer, values['peer-check']);
	if (peer !== undefined && values.label === undefined) {
		throw new Error('--label names the application under test, which --peer measures');
	}
	const label = (values.label ?? LOCAL_LABEL).trim();
	if (!label || !/^[\w .-]{1,40}$/.test(label)) {
		throw new Error('--label names what is measured (letters, digits, space, dot, dash)');
	}
	const waitSeconds = Number(values.wait);
	if (!Number.isInteger(waitSeconds) || waitSeconds < 10 || waitSeconds > 1800) {
		throw new Error('--wait must be 10..1800 seconds');
	}
	const stamp = new Date().toISOString().replaceAll(':', '-');
	return {
		peer,
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
	/** The same over the circle's first and last two seconds: a lag that builds up shows as the difference. */
	readonly pointerTrailEarly: Spread | undefined;
	readonly pointerTrailLate: Spread | undefined;
	/** Pictures during the circle showing none of the last half second's positions. */
	readonly pointerTrailLost: number;
	/** Keys typed one every 250 ms, and in a burst one every 30 ms. */
	readonly keys: KeysSummary | undefined;
	readonly keyBurst: KeysSummary | undefined;
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
	/** Whether the page stopped changing after the freeze; undefined without one. */
	readonly frozen: boolean | undefined;
	/** SSIM of every picture of the card after the freeze, ms after it. */
	readonly stillSharpness: readonly { readonly afterMs: number; readonly ssim: number }[];
	/** SSIM of the last picture of the still card, and when the card first came within 0.005 of it. */
	readonly stillSsim: number | undefined;
	readonly stillSharpAfterMs: number | undefined;
}

/** What a run of key presses showed. */
export interface KeysSummary {
	readonly pressed: number;
	/** Keys the page counted over the run; above `pressed` some were doubled, below some were lost. */
	readonly shown: number | undefined;
	/** Whether the page's text came out as typed. */
	readonly textRight: boolean | undefined;
	/** From a key going down to the first picture counting it, ms. */
	readonly latency: Spread | undefined;
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
		.map(event => ({
			t: number(event.t) ?? 0,
			frame: number(event.frame),
			clicks: number(event.clicks),
			x: number(event.x),
			y: number(event.y),
			keys: number(event.keys),
			typed: number(event.typed),
		}));
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
	// Each picture during the circle against the move it shows, and the
	// pictures that show none of the last half second's.
	const trail: { readonly t: number; readonly lag: number }[] = [];
	let trailLost = 0;
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
				trail.push({ t: frame.t, lag: frame.t - (number(shown.t) ?? 0) });
			} else {
				trailLost++;
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
	// Keys: each press against the first picture that counts it, and
	// whether each run ended with every key counted once and the text right.
	const presses = events.filter(event => event.type === 'key');
	const typing = (phase: string): KeysSummary | undefined => {
		const run = presses.filter(press => press.phase === phase);
		if (!run.length) {
			return undefined;
		}
		const latency: number[] = [];
		run.forEach(press => {
			const t = number(press.t) ?? 0;
			const keys = number(press.keys) ?? 0;
			const hit = after(t, t + 2000, frame => frame.keys !== undefined && frame.keys >= keys);
			if (hit) {
				latency.push(hit.t - t);
			}
		});
		const last = run[run.length - 1];
		const end = number(last.t) ?? 0;
		const shown = frames.filter(frame => frame.t > end && frame.t <= end + 1000 && frame.keys !== undefined).at(-1);
		const expected = (number(last.keys) ?? 0) - (number(run[0].keys) ?? 0) + 1;
		const before = frames.filter(frame => frame.t <= (number(run[0].t) ?? 0) && frame.keys !== undefined).at(-1)?.keys ?? 0;
		return {
			pressed: expected,
			shown: shown?.keys === undefined ? undefined : shown.keys - before,
			textRight: shown ? shown.typed === number(last.typed) : undefined,
			latency: spread(latency),
		};
	};
	// Pacing: the pictures after the clicks and keys, the pointer resting.
	const settledFrom = Math.max(number(clicks.at(-1)?.t) ?? 0, number(presses.at(-1)?.t) ?? 0) + 500;
	const freeze = events.find(event => event.type === 'freeze');
	const frozenAt = number(freeze?.t) ?? Infinity;
	const pacing = frames.filter(frame => frame.t > settledFrom && frame.t < frozenAt);
	const intervals = pacing.slice(1).map((frame, index) => frame.t - pacing[index].t);
	const counters = pacing.map(frame => frame.frame).filter((value): value is number => value !== undefined);
	const span = pacing.length > 1 ? (pacing[pacing.length - 1].t - pacing[0].t) / 1000 : 0;
	const hostFrames = counters.length > 1 ? counters[counters.length - 1] - counters[0] + 1 : 0;
	// Sharpness after each swap, by the scheduled time it was asked at.
	const scored = events.filter(event => event.type === 'crop' && typeof event.file === 'string' && ssim.has(event.file));
	const crops = scored.filter(event => event.phase !== 'still');
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
	// The still screen: every picture after the freeze, in order. The page
	// froze when the frame counter stops a second after the click.
	const still = scored.filter(event => event.phase === 'still')
		.map(crop => ({ afterMs: Math.round(number(crop.sinceMs) ?? 0), ssim: ssim.get(crop.file as string) ?? 0 }));
	const stillSsim = still.at(-1)?.ssim;
	const stillFrames = frames.filter(frame => frame.t > frozenAt + 1000 && frame.frame !== undefined);
	return {
		label,
		scale: number(located.scaleX) ?? 0,
		host: { width: number(located.hostWidth) ?? 0, height: number(located.hostHeight) ?? 0 },
		viewer: { width: number(located.viewerWidth) ?? 0, height: number(located.viewerHeight) ?? 0 },
		restarts: all.filter(event => event.type === 'moved').length,
		pointerJump: spread(jumpLatency),
		pointerMissed,
		pointerError: spread(jumpError),
		pointerTrail: spread(trail.map(point => point.lag)),
		pointerTrailEarly: spread(trail.filter(point => point.t < (number(path[0]?.t) ?? 0) + 2000).map(point => point.lag)),
		pointerTrailLate: spread(trail.filter(point => point.t > (number(path.at(-1)?.t) ?? 0) - 2000).map(point => point.lag)),
		pointerTrailLost: trailLost,
		keys: typing('paced'),
		keyBurst: typing('burst'),
		click: spread(clickLatency),
		clicksMissed,
		framesPerSecond: span > 0 ? (pacing.length - 1) / span : undefined,
		frameInterval: spread(intervals),
		stutters: intervals.filter(interval => interval > 50).length,
		hostFramesShown: hostFrames > 0 ? new Set(counters).size / hostFrames : undefined,
		sharpness,
		settledSsim,
		sharpAfterMs: settledSsim === undefined ? undefined : sharpness.find(point => point.ssim >= settledSsim - 0.01)?.afterMs,
		frozen: freeze ? stillFrames.length > 0 && stillFrames.every(frame => frame.frame === stillFrames[0].frame) : undefined,
		stillSharpness: still,
		stillSsim,
		stillSharpAfterMs: stillSsim === undefined ? undefined : still.find(point => point.ssim >= stillSsim - 0.005)?.afterMs,
	};
}

const ms = (spread: Spread | undefined): string => spread ? `${spread.p50.toFixed(0)} / ${spread.p95.toFixed(0)}` : '-';

const keyRow = (keys: KeysSummary | undefined): string => keys
	? `${keys.shown === undefined ? '-' : String(keys.shown)} / ${String(keys.pressed)}, ${keys.textRight === undefined ? '-' : keys.textRight ? 'yes' : 'no'}`
	: '-';

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
		row('Pointer trail, first / last 2 s, p50 ms', run => `${run.pointerTrailEarly ? run.pointerTrailEarly.p50.toFixed(0) : '-'} / ${run.pointerTrailLate ? run.pointerTrailLate.p50.toFixed(0) : '-'}${run.pointerTrailLost ? ` (${String(run.pointerTrailLost)} pictures over 500 ms behind)` : ''}`),
		row('Key to picture, p50 / p95 ms', run => ms(run.keys?.latency)),
		row('Keys shown / pressed, text right', run => keyRow(run.keys)),
		row('Fast typing: key to picture, p50 / p95 ms', run => ms(run.keyBurst?.latency)),
		row('Fast typing: keys shown / pressed, text right', run => keyRow(run.keyBurst)),
		row('Click to picture, p50 / p95 ms', run => `${ms(run.click)}${run.clicksMissed ? ` (${String(run.clicksMissed)} missed)` : ''}`),
		row('Pictures a second', run => fixed(run.framesPerSecond, 1)),
		row('Picture interval, p50 / p95 ms', run => ms(run.frameInterval)),
		row('Stutters (gaps over 50 ms)', run => String(run.stutters)),
		row('Host frames shown', run => run.hostFramesShown === undefined ? '-' : `${(run.hostFramesShown * 100).toFixed(0)}%`),
		row('Card SSIM settled', run => fixed(run.settledSsim, 4)),
		row('Card sharp after a swap, ms', run => run.sharpAfterMs === undefined ? '-' : String(run.sharpAfterMs)),
		row('Still screen: card SSIM first / last', run => run.frozen === false ? 'page did not freeze' : run.stillSharpness.length ? `${fixed(run.stillSharpness[0].ssim, 4)} / ${fixed(run.stillSsim, 4)}` : '-'),
		row('Still screen: last change, ms after the freeze', run => run.stillSharpAfterMs === undefined ? '-' : String(run.stillSharpAfterMs)),
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
	const all = lines.split('\n').filter(line => line.trim()).map(line => JSON.parse(line) as Event);
	// Only the measurement after the last lock counts, as in the summary.
	const events = all.slice(Math.max(0, all.findLastIndex(event => event.type === 'located')));
	const located = events.at(0);
	if (located?.type !== 'located') {
		throw new Error('The driver never found the page on this screen');
	}
	const cell = number(located.cell) ?? 12;
	const layout = perceivedLayout(number(located.hostWidth) ?? 0, number(located.hostHeight) ?? 0, cell / 12);
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

/** The stimulus page, running until the run stops it. */
interface Stimulus {
	/** What to tell the person at this Mac before the driver starts. */
	readonly instructions: string;
	/** Rejects when the page quits before the run ends. */
	readonly exited: Promise<never>;
	stop(): Promise<void>;
}

/**
 * The page in the development Electron `npm run build` prepared, as a child
 * of `processes`: it needs no permission of its own, and stopping them
 * stops it. On Linux it runs under XWayland, where it can be maximised
 * above every window.
 */
async function startStimulus(root: string, processes: ProcessRunner, signal: AbortSignal): Promise<LaunchedProcess> {
	const entry = join(root, 'out/perceived.cjs');
	await access(entry).catch(() => {
		throw new Error('out/perceived.cjs is missing: run npm run build first');
	});
	return processes.start(await electronExecutable(root, signal), [...isLinux ? ['--ozone-platform=x11'] : [], entry], { stdio: 'ignore' });
}

/** The page on this Mac, as a child of this task. */
async function localStimulus(root: string, options: PerceivedOptions, processes: ProcessRunner, signal: AbortSignal): Promise<Stimulus> {
	const page = await startStimulus(root, processes, signal);
	const exited = page.result.then(
		() => Promise.reject(new Error('The stimulus window closed before the run finished')),
		(error: unknown) => Promise.reject(new Error(`The stimulus stopped: ${error instanceof Error ? error.message : String(error)}`)),
	);
	exited.catch(() => undefined);
	return {
		instructions: `The stimulus is maximised on this Mac. Leave it in front and the mouse and keyboard alone: measuring starts when the page is found (up to ${String(options.waitSeconds)} s), then the pointer moves and keys are typed by themselves for about a minute.`,
		exited,
		stop: () => Promise.resolve(),
	};
}

/**
 * The page on the computer being controlled, started by the test agent
 * there from its own checkout (the `perceived-stimulus` scenario), and
 * watched until the run stops it.
 */
async function agentStimulus(root: string, options: PerceivedOptions, peer: AgentPeer, signal: AbortSignal): Promise<Stimulus> {
	const session = await openAgentSession(root, peer, signal);
	try {
		await session.start('perceived-stimulus');
	} catch (error) {
		await session.close();
		throw error;
	}
	// Set by `stop` while the watch waits.
	const state = { stopped: false };
	const exited = (async (): Promise<never> => {
		while (!state.stopped) {
			await new Promise(resolve => setTimeout(resolve, 2000));
			const polled = await session.poll().catch((error: unknown) => {
				if (state.stopped) {
					return { running: true };
				}
				throw error;
			});
			// After `stop` nobody waits on this any more.
			if (!polled.running) {
				throw new Error('The stimulus on the agent\'s computer quit before the run finished');
			}
		}
		return new Promise<never>(() => undefined);
	})();
	exited.catch(() => undefined);
	return {
		instructions: `The stimulus is maximised on the agent's computer (${session.platform.os}). Open ${options.label} on this Mac to it now, full screen, showing its whole screen; measuring starts when the page is found (up to ${String(options.waitSeconds)} s), then the pointer moves by itself for about a minute.`,
		exited,
		stop: async () => {
			state.stopped = true;
			await session.stop().catch(() => undefined);
			await session.close();
		},
	};
}

/**
 * The page alone on the test agent's computer (`testkit:launch
 * perceived-stimulus`), until `stopped` settles.
 */
export async function launchPerceivedStimulus(root: string, signal: AbortSignal, stopped: Promise<void>): Promise<void> {
	await assertBundleFresh(root, 'perceived.cjs');
	const processes = new ProcessRunner(signal);
	try {
		const page = await startStimulus(root, processes, signal);
		await Promise.race([page.result, stopped]);
	} finally {
		await processes.stop();
	}
}

export async function measurePerceived(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	if (args.includes('--help')) {
		console.log(perceivedUsage);
		return;
	}
	if (process.platform !== 'darwin') {
		throw new Error('measure:perceived views from macOS');
	}
	const options = perceivedOptions(root, args);
	const processes = new ProcessRunner(signal);
	let stimulus: Stimulus | undefined;
	try {
		await mkdir(options.output, { recursive: true });
		await processes.run('cargo', ['build', '--release', '--locked', '-p', 'vnilink-macos', '--example', 'perceived'], { cwd: root });
		const pixels = await displayPixels(signal);
		stimulus = options.peer
			? await agentStimulus(root, options, options.peer, signal)
			: await localStimulus(root, options, processes, signal);
		console.log(stimulus.instructions);
		await Promise.race([
			processes.run(join(root, 'target/release/examples/perceived'), [options.output, String(options.waitSeconds), ...(pixels ? [String(pixels)] : [])]),
			stimulus.exited,
		]);
		await analysePerceived(options.output, options.label, signal);
	} finally {
		await stimulus?.stop();
		await processes.stop();
	}
}

/** Runs measured before, side by side: `--compare <dir> <dir>...`. */
export async function comparePerceived(directories: readonly string[]): Promise<void> {
	const runs = await Promise.all(directories.map(async directory => JSON.parse(await readFile(join(directory, 'summary.json'), 'utf8')) as PerceivedSummary));
	console.log(renderPerceived(runs));
}
