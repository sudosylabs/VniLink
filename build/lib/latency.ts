// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Glass-to-glass latency on one Mac, Windows or Linux computer: a host and
 * a viewer, each a temporary VniLink with its own identity, stream this
 * computer's screen to each other over loopback. The host shows a strip
 * carrying its frame time across the top of the main display; the viewer
 * shows the stream below it; a reader (ScreenCaptureKit on macOS, Desktop
 * Duplication on Windows, the ScreenCast portal on Linux) decodes both
 * strips from the same picture.
 *
 * It proves capture-to-composited-picture latency through the real path:
 * the platform's capture and encoder, QUIC, the utility process, WebCodecs
 * and the canvas. It does not cover a network, panel response (both strips
 * share it), another computer or another operating system, and host and
 * viewer share one CPU and GPU.
 */
import { mkdir, mkdtemp, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { closeSync, existsSync, openSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { assertBundleFresh } from './bundles.ts';
import { electronExecutable } from './electron.ts';
import { assertNativeFresh, defaultNativeOptions } from './native.ts';
import { osName } from './platform.ts';
import { type LaunchedProcess, ProcessRunner } from './processes.ts';
import { agentPeer, openAgentSession, type AgentPeer, type AgentSession } from './remote.ts';

export interface LatencyOptions {
	/** How long the reader samples, after the stream has settled. */
	readonly seconds: number;
	/** Settling time between the first picture and sampling. */
	readonly settleSeconds: number;
	readonly output: string;
	/**
	 * macOS: a development VniLink that already holds Screen Recording, such
	 * as another checkout's. macOS grants an ad-hoc signed copy by its code
	 * hash, so each checkout's copy needs its own grant. The application
	 * only hosts Electron; the code it runs is this checkout's `out/`.
	 */
	readonly app: string | undefined;
	/**
	 * Hold mode, in seconds; 0 when off. The stimulus moves for 2 s and
	 * holds for this long, over and over, above a panel of coloured text,
	 * so the host sends still pictures; the summary adds when each showed
	 * and what the first change after a hold took.
	 */
	readonly holdSeconds: number;
	/** The viewer's frame rate at the Smooth preset's bitrate and size; undefined keeps the default quality. */
	readonly frameRate: number | undefined;
	/** The viewer's bitrate limit in kb/s with `--fps`; undefined keeps the Smooth preset's 12 Mb/s. */
	readonly bitrate: number | undefined;
	/** The stimulus fills the screen below the strip with detail that changes every frame. */
	readonly busy: boolean;
	/** Two computers: the test agent on the other one; undefined runs both sides on this one. */
	readonly peer: LatencyPeer | undefined;
}

/** The other computer of a two-computer run: the test agent there, built from the same commit. */
export interface LatencyPeer extends AgentPeer {
	/** Which computer hosts its screen: this one, or the agent's. */
	readonly hosts: 'here' | 'peer';
}

export function latencyOptions(root: string, args: readonly string[]): LatencyOptions {
	const { values } = parseArgs({
		args: [...args],
		options: {
			seconds: { type: 'string', default: '20' }, settle: { type: 'string', default: '3' }, output: { type: 'string' }, app: { type: 'string' }, hold: { type: 'string', default: '0' },
			fps: { type: 'string' }, bitrate: { type: 'string' }, busy: { type: 'boolean', default: false }, peer: { type: 'string' }, 'peer-check': { type: 'string' }, hosts: { type: 'string', default: 'here' },
		},
		strict: true,
	});
	const seconds = Number(values.seconds);
	const settleSeconds = Number(values.settle);
	if (!Number.isInteger(seconds) || seconds < 1 || seconds > 600 || !Number.isFinite(settleSeconds) || settleSeconds < 0) {
		throw new Error('--seconds must be 1..600 and --settle at least 0');
	}
	const holdSeconds = Number(values.hold);
	if (!Number.isInteger(holdSeconds) || holdSeconds < 0 || holdSeconds > 60) {
		throw new Error('--hold must be 0..60 seconds');
	}
	if (values.app !== undefined && !/\.app\/?$/.test(values.app)) {
		throw new Error('--app must name a .app bundle');
	}
	const frameRate = values.fps === undefined ? undefined : Number(values.fps);
	if (frameRate !== undefined && (!Number.isInteger(frameRate) || frameRate < 1 || frameRate > 120)) {
		throw new Error('--fps must be 1..120');
	}
	const bitrate = values.bitrate === undefined ? undefined : Number(values.bitrate);
	if (bitrate !== undefined && (!Number.isInteger(bitrate) || bitrate < 1 || bitrate > 50_000)) {
		throw new Error('--bitrate must be 1..50000 kb/s');
	}
	const agent = agentPeer(values.peer, values['peer-check']);
	if (values.busy && agent === undefined) {
		throw new Error('--busy needs --peer: the stimulus then fills the screen, and no viewer shares it');
	}
	if (values.hosts !== 'here' && values.hosts !== 'peer') {
		throw new Error('--hosts must be here or peer');
	}
	if (agent === undefined && values.hosts !== 'here') {
		throw new Error('--hosts needs --peer');
	}
	if (agent !== undefined && holdSeconds > 0) {
		throw new Error('--hold reads one screen and cannot run across two computers');
	}
	const peer = agent === undefined ? undefined : { ...agent, hosts: values.hosts === 'peer' ? 'peer' as const : 'here' as const };
	// Absolute: the two sides run from another directory.
	const output = resolve(values.output ?? join(root, '.build/latency', new Date().toISOString().replaceAll(':', '-')));
	return { seconds, settleSeconds, output, app: values.app, holdSeconds, frameRate, bitrate, busy: values.busy, peer };
}

/** Each stage one side measures of itself, sampled once a second: its p50, p95 and samples. */
export interface StagePercentiles {
	readonly p50: number;
	readonly p95: number;
	readonly count: number;
}

/** A pipeline over a run, from one side's once-a-second diagnostics samples (`PipelineSummary`). */
export interface PipelineRun {
	/** Host: capture and encode times, scaling and copies included in encode, in ms. */
	readonly capture: StagePercentiles | undefined;
	readonly encode: StagePercentiles | undefined;
	/** The last sample's codec, size, encoder or decoder and counters. */
	readonly last: Readonly<Record<string, unknown>>;
	/** Every condition any sample held. */
	readonly conditions: readonly string[];
}

/** What the viewer's status line and stage text said over a run, once a second. */
export interface ViewerStatus {
	/** Pictures received a second. */
	readonly frameRate: StagePercentiles | undefined;
	/** The round trip, in ms: the network leg, as one-way times are not read. */
	readonly roundTrip: StagePercentiles | undefined;
	/** Pictures a second replaced by newer ones before the window drew them. */
	readonly replaced: StagePercentiles | undefined;
}

/** The viewer's stage lines (`{ text, status }`) as its frame rate, round trip and replaced pictures. */
export function summariseViewerStatus(stages: string): ViewerStatus {
	const entries = lines(stages).flatMap(entry => typeof entry === 'object' && entry !== null ? [entry as Record<string, unknown>] : []);
	const read = (field: string, pattern: RegExp): StagePercentiles | undefined => stagePercentiles(entries.flatMap(entry => {
		const match = typeof entry[field] === 'string' ? pattern.exec(entry[field]) : null;
		return match ? [Number(match[1])] : [];
	}));
	// "50 fps · H.264 · 380 kb/s · 76 ms"; "17 pictures replaced by newer ones before drawing"
	return { frameRate: read('status', /(\d+) fps/), roundTrip: read('status', /(\d+) ms$/), replaced: read('text', /(\d+) pictures replaced/) };
}

function stagePercentiles(values: readonly number[]): StagePercentiles | undefined {
	const sorted = [...values].sort((a, b) => a - b);
	return sorted.length ? { p50: rank(sorted, 0.5), p95: rank(sorted, 0.95), count: sorted.length } : undefined;
}

/** A side's pipeline samples (`{ at, sample }` lines) as one run. */
export function summarisePipeline(samples: string): PipelineRun {
	const pipelines = lines(samples).flatMap(line => typeof line === 'object' && line !== null && 'sample' in line && typeof line.sample === 'object' && line.sample !== null
		? [line.sample as Record<string, unknown>]
		: []);
	const ms = (name: string, scale: number): StagePercentiles | undefined => stagePercentiles(pipelines.flatMap(pipeline => {
		const value = pipeline[name];
		return typeof value === 'number' ? [value / scale] : [];
	}));
	const conditions = new Set(pipelines.flatMap(pipeline => Array.isArray(pipeline.conditions)
		? pipeline.conditions.flatMap((condition: unknown) => typeof condition === 'object' && condition !== null && 'condition' in condition && typeof condition.condition === 'string' ? [condition.condition] : [])
		: []));
	return { capture: ms('capture_us', 1000), encode: ms('encode_us', 1000), last: pipelines.at(-1) ?? {}, conditions: [...conditions] };
}

/** A two-computer run's summary: each side's own stage times; no glass-to-glass total, as no one screen shows both. */
export function renderAcrossSummary(
	host: PipelineRun, viewer: PipelineRun, stages: LatencySummary, status: ViewerStatus, cpu?: LatencyCpu,
): string {
	const row = (name: string, value: StagePercentiles | undefined): string => value
		? `| ${name} | ${value.p50.toFixed(1)} | ${value.p95.toFixed(1)} | ${String(value.count)} |`
		: `| ${name} | - | - | 0 |`;
	const field = (pipeline: PipelineRun, name: string): string => {
		const value = pipeline.last[name];
		return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : 'unknown';
	};
	return [
		'# Stage times across two computers',
		'',
		`Codec ${field(host, 'codec')}, ${field(host, 'width')}x${field(host, 'height')}; host encoder ${field(host, 'encoder')} (hardware ${field(host, 'encoder_hardware')}, input ${field(host, 'encoder_input')}); viewer decoder hardware ${field(viewer, 'decoder_hardware')}.`,
		'',
		'| Host, once a second | p50 ms | p95 ms | Samples |',
		'|---|---|---|---|',
		row('Capture', host.capture),
		row('Encode (scaling and copies included)', host.encode),
		'',
		`Host counters at the end: keyframes ${field(host, 'keyframes_sent')}, encoder reopens ${field(host, 'encoder_reopens')}, frames dropped before sending ${field(host, 'send_dropped')}.`,
		'',
		'| Viewer | p50 | p95 | Samples |',
		'|---|---|---|---|',
		row('Round trip (ms)', status.roundTrip),
		row('Pictures received a second', status.frameRate),
		row('Pictures a second replaced before drawing', status.replaced),
		'',
		'Stages the viewer measures (median of one-second windows); its host stage is capture to send on the host:',
		'',
		'| Stage | p50 ms | p95 ms | Windows |',
		'|---|---|---|---|',
		...[...stages.stages].map(([name, stage]) => `| ${name} | ${stage.p50.toFixed(1)} | ${stage.p95.toFixed(1)} | ${String(stage.windows)} |`),
		'',
		`Conditions the viewer held: ${viewer.conditions.length ? viewer.conditions.join(', ') : 'none'}.`,
		'',
		...(cpu ? [
			'CPU over the run, in cores:',
			'',
			'| Side | Total | By process |',
			'|---|---|---|',
			...(['host', 'viewer'] as const).map(side => {
				const kinds = Object.entries(cpu[side]).sort(([, a], [, b]) => b - a);
				const total = kinds.reduce((sum, [, used]) => sum + used, 0);
				return `| ${side} | ${total.toFixed(2)} | ${kinds.map(([kind, used]) => `${kind} ${used.toFixed(2)}`).join(', ')} |`;
			}),
			'',
		] : []),
	].join('\n');
}

export interface Percentiles {
	readonly p50: number;
	readonly p95: number;
	readonly p99: number;
	readonly max: number;
	readonly count: number;
}

export interface LatencySummary {
	readonly pictures: number;
	/** Glass to glass, read from the screen. */
	readonly total: Percentiles | undefined;
	/** Each stage the viewer measures itself: the median over its one-second windows of their p50 and p95. */
	readonly stages: ReadonlyMap<string, { readonly p50: number; readonly p95: number; readonly windows: number }>;
}

/** The nearest-rank percentile of sorted values. */
function rank(sorted: readonly number[], fraction: number): number {
	return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(fraction * sorted.length) - 1))];
}

function lines(text: string): unknown[] {
	return text.split('\n').filter(line => line.trim().length > 0).map(line => JSON.parse(line) as unknown);
}

/** The reader's samples and the viewer's stage lines, as one summary. */
export function summariseLatency(samples: string, stages: string): LatencySummary {
	const pictures = lines(samples);
	const totals = pictures
		.map(sample => typeof sample === 'object' && sample !== null && 'latencyMs' in sample ? sample.latencyMs : null)
		.filter((value): value is number => typeof value === 'number')
		.sort((a, b) => a - b);
	const total = totals.length
		? { p50: rank(totals, 0.5), p95: rank(totals, 0.95), p99: rank(totals, 0.99), max: totals[totals.length - 1], count: totals.length }
		: undefined;
	const windows = new Map<string, { p50: number[]; p95: number[] }>();
	for (const entry of lines(stages)) {
		const text = typeof entry === 'object' && entry !== null && 'text' in entry && typeof entry.text === 'string'
			? entry.text
			: '';
		for (const line of text.split('\n')) {
			// "Decoding: 1.2 ms, 3.4 ms at most for 95%"
			const match = /^(.+): ([\d.]+) ms, ([\d.]+) ms at most for 95%$/.exec(line);
			if (match) {
				const stage = windows.get(match[1]) ?? { p50: [], p95: [] };
				stage.p50.push(Number(match[2]));
				stage.p95.push(Number(match[3]));
				windows.set(match[1], stage);
			}
		}
	}
	const median = (values: number[]): number => rank([...values].sort((a, b) => a - b), 0.5);
	return {
		pictures: pictures.length,
		total,
		stages: new Map([...windows].map(([name, stage]) => [name, { p50: median(stage.p50), p95: median(stage.p95), windows: stage.p50.length }])),
	};
}

/** When the stimulus moves and holds: from `origin` (wall-clock ms), `moveMs` then `holdMs`, over and over. */
export interface HoldSchedule {
	readonly origin: number;
	readonly moveMs: number;
	readonly holdMs: number;
}

/** Hold mode's results: when each still showed, and what the first change after a hold took. */
export interface HoldSummary {
	readonly holds: number;
	/** From the stimulus holding to the viewer showing a still, for each hold that brought one. */
	readonly stillAfterMs: readonly number[];
	/** Glass to glass of the first picture read after each hold, against every other picture. */
	readonly afterHold: Percentiles | undefined;
	readonly others: Percentiles | undefined;
}

function percentiles(values: number[]): Percentiles | undefined {
	const sorted = [...values].sort((a, b) => a - b);
	return sorted.length
		? { p50: rank(sorted, 0.5), p95: rank(sorted, 0.95), p99: rank(sorted, 0.99), max: sorted[sorted.length - 1], count: sorted.length }
		: undefined;
}

/**
 * Hold mode's summary from the stimulus `schedule` (`origin`, `moveMs`,
 * `holdMs`, wall-clock ms), the viewer's `stills` lines (`at`, `shown`) and
 * the reader's `samples`. A hold shows in the samples as a second or more
 * of the reader's time in which the newest strip does not change, however
 * many pictures the capture delivers meanwhile (macOS none, Windows a few a
 * second); the first picture with both strips after it is the first change
 * after the hold.
 */
export function summariseHolds(schedule: HoldSchedule, stills: string, samples: string): HoldSummary {
	const shown = lines(stills).filter((line): line is { at: number; shown: number } => typeof line === 'object' && line !== null
		&& 'at' in line && typeof line.at === 'number' && 'shown' in line && typeof line.shown === 'number');
	const cycle = schedule.moveMs + schedule.holdMs;
	const stillAfterMs: number[] = [];
	let holds = 0;
	if (shown.length) {
		const first = shown[0].at;
		const last = shown[shown.length - 1].at;
		for (let start = schedule.origin + schedule.moveMs; start + schedule.holdMs <= last; start += cycle) {
			if (start < first) {
				continue;
			}
			holds++;
			const before = shown.filter(line => line.at <= start).at(-1)?.shown ?? 0;
			const came = shown.find(line => line.at > start && line.at < start + schedule.holdMs && line.shown > before);
			if (came) {
				stillAfterMs.push(came.at - start);
			}
		}
	}
	const pictures = lines(samples).flatMap(line => {
		if (typeof line !== 'object' || line === null || !('codes' in line) || !Array.isArray(line.codes)) {
			return [];
		}
		const values = line.codes.flatMap((code: unknown) => typeof code === 'object' && code !== null && 'value' in code && typeof code.value === 'number' ? [code.value] : []);
		const latency = 'latencyMs' in line && typeof line.latencyMs === 'number' ? line.latencyMs : undefined;
		const shown = 'shownMs' in line && typeof line.shownMs === 'number' ? line.shownMs : undefined;
		return values.length ? [{ newest: Math.max(...values), latency, shown }] : [];
	});
	const afterHold: number[] = [];
	const others: number[] = [];
	// When the newest strip last changed, on the reader's clock: a second
	// without a change is a hold.
	let since: number | undefined;
	let waiting = false;
	for (let index = 0; index < pictures.length; index++) {
		const picture = pictures[index];
		const changed = index > 0 && picture.newest !== pictures[index - 1].newest;
		if (changed && since !== undefined && picture.shown !== undefined && picture.shown - since >= 1000) {
			waiting = true;
		}
		if (changed || index === 0) {
			since = picture.shown;
		}
		if (picture.latency === undefined) {
			continue;
		}
		if (waiting && changed) {
			afterHold.push(picture.latency);
			waiting = false;
		} else if (!waiting) {
			others.push(picture.latency);
		}
	}
	return { holds, stillAfterMs, afterHold: percentiles(afterHold), others: percentiles(others) };
}

export function renderHoldSummary(hold: HoldSummary): string {
	const after = [...hold.stillAfterMs].sort((a, b) => a - b);
	const row = (name: string, value: Percentiles | undefined): string => value
		? `| ${name} | ${String(value.p50)} | ${String(value.p95)} | ${String(value.max)} | ${String(value.count)} |`
		: `| ${name} | - | - | - | 0 |`;
	return [
		'## Still pictures (hold mode)',
		'',
		`Holds: ${String(hold.holds)}; a still showed in ${String(after.length)}${after.length ? `, after ${String(after[0])} to ${String(after[after.length - 1])} ms (median ${String(rank(after, 0.5))} ms)` : ''}.`,
		'',
		'| Glass to glass | p50 ms | p95 ms | max ms | Pictures |',
		'|---|---|---|---|---|',
		row('First change after a hold', hold.afterHold),
		row('Every other picture', hold.others),
		'',
	].join('\n');
}

/** Cores each side's processes used while the reader ran, by kind. */
export interface LatencyCpu {
	readonly host: Readonly<Record<string, number>>;
	readonly viewer: Readonly<Record<string, number>>;
}

/** The encoder the host had open, as its diagnostics recorded it. */
export interface LatencyEncoder {
	readonly encoder?: string;
	readonly hardware?: boolean;
	readonly input?: string;
}

/** The encoder in the host's pipeline summary (`PipelineSummary`), keeping only fields of the expected types. */
function latencyEncoder(pipeline: unknown): LatencyEncoder {
	if (typeof pipeline !== 'object' || pipeline === null) {
		return {};
	}
	const field = (name: string): unknown => (pipeline as Record<string, unknown>)[name];
	const text = (name: string): string | undefined => typeof field(name) === 'string' ? field(name) as string : undefined;
	return { encoder: text('encoder'), hardware: typeof field('encoder_hardware') === 'boolean' ? field('encoder_hardware') as boolean : undefined, input: text('encoder_input') };
}

export function renderLatencySummary(summary: LatencySummary, cpu?: LatencyCpu, encoder?: LatencyEncoder): string {
	const rows = [
		'# Glass-to-glass latency',
		'',
		`Pictures read: ${String(summary.pictures)}; with both strips: ${String(summary.total?.count ?? 0)}.`,
		'',
		...(encoder ? [`Host encoder: ${encoder.encoder ?? 'unknown'}, ${encoder.hardware === true ? 'hardware' : encoder.hardware === false ? 'software' : 'kind unknown'}, input ${encoder.input ?? 'unknown'}.`, ''] : []),
		'| | p50 ms | p95 ms | p99 ms | max ms |',
		'|---|---|---|---|---|',
		summary.total ? `| Glass to glass | ${String(summary.total.p50)} | ${String(summary.total.p95)} | ${String(summary.total.p99)} | ${String(summary.total.max)} |` : '| Glass to glass | - | - | - | - |',
		'',
		'Stages the viewer measures (median of one-second windows):',
		'',
		'| Stage | p50 ms | p95 ms | Windows |',
		'|---|---|---|---|',
		...[...summary.stages].map(([name, stage]) => `| ${name} | ${stage.p50.toFixed(1)} | ${stage.p95.toFixed(1)} | ${String(stage.windows)} |`),
		'',
		...(cpu ? [
			'CPU while the reader ran, in cores (the reader itself not included):',
			'',
			'| Side | Total | By process |',
			'|---|---|---|',
			...(['host', 'viewer'] as const).map(side => {
				const kinds = Object.entries(cpu[side]).sort(([, a], [, b]) => b - a);
				const total = kinds.reduce((sum, [, used]) => sum + used, 0);
				return `| ${side} | ${total.toFixed(2)} | ${kinds.map(([kind, used]) => `${kind} ${used.toFixed(2)}`).join(', ')} |`;
			}),
			'',
		] : []),
	];
	return rows.join('\n');
}

/** Waits until `work` settles or `ms` pass, whichever is first, leaving no timer behind to keep the process alive. */
async function settledWithin(work: Promise<unknown>, ms: number): Promise<void> {
	let timer: NodeJS.Timeout | undefined;
	try {
		await Promise.race([work.catch(() => undefined), new Promise(resolve => {
			timer = setTimeout(resolve, ms);
		})]);
	} finally {
		clearTimeout(timer);
	}
}

/** Publishes `value` under `name` for the sides, whole or not at all. */
async function publish(directory: string, name: string, value: unknown): Promise<void> {
	const path = join(directory, `${name}.json`);
	await writeFile(`${path}.partial`, JSON.stringify(value));
	await rename(`${path}.partial`, path);
}

/** Waits for `name` in the rendezvous directory, or fails when either side has exited. */
async function arrival(directory: string, name: string, timeoutMs: number, exited: () => boolean, signal: AbortSignal): Promise<void> {
	const deadline = Date.now() + timeoutMs;
	while (!existsSync(join(directory, `${name}.json`))) {
		signal.throwIfAborted();
		if (exited()) {
			throw new Error(`A side exited before ${name}`);
		}
		if (Date.now() > deadline) {
			throw new Error(`${name} did not arrive within ${String(timeoutMs / 1000)} s`);
		}
		await new Promise(resolve => setTimeout(resolve, 200));
	}
}

/** How one platform runs the two sides and the reader. */
interface LatencyPlatform {
	/** The crate whose `glass_latency` example reads this platform's display. */
	readonly readerCrate: string;
	readonly reader: string;
	/** How long someone at this computer may take to answer a consent dialog, for the host and for the reader. */
	readonly consentMs: number;
	/** Starts one side; its result settles when the side has quit. */
	launch(role: 'host' | 'viewer', environment: Readonly<Record<string, string>>, log: string): LaunchedProcess;
}

/** `app`: on macOS, the `.app` that runs the sides; this checkout's development VniLink when undefined. */
async function latencyPlatform(root: string, app: string | undefined, processes: ProcessRunner, signal: AbortSignal): Promise<LatencyPlatform> {
	const entry = join(root, 'out/latency.cjs');
	if (process.platform === 'darwin') {
		// .../VniLink.app/Contents/MacOS/Electron
		const bundle = app ?? dirname(dirname(dirname(await electronExecutable(root, signal))));
		return {
			readerCrate: 'vnilink-macos',
			reader: join(root, 'target/release/examples/glass_latency'),
			// macOS asks before the run; a refusal stops it.
			consentMs: 0,
			// Launched through LaunchServices, each side is its own responsible
			// application, so macOS asks Screen Recording of the development
			// VniLink and not of whatever started this task. `open -W` waits for it.
			launch: (_role, environment, log) => processes.start('open', [
				'-n', '-W', '-g', '-a', bundle,
				...Object.entries(environment).flatMap(([name, value]) => ['--env', `${name}=${value}`]),
				'--stdout', log, '--stderr', log,
				'--args', entry,
			]),
		};
	}
	if (app !== undefined) {
		throw new Error('--app names a macOS application');
	}
	const electron = await electronExecutable(root, signal);
	const direct = (args: (role: 'host' | 'viewer', environment: Readonly<Record<string, string>>) => readonly string[]): LatencyPlatform['launch'] => (role, environment, log) => {
		const output = openSync(log, 'a');
		try {
			return processes.start(electron, [...args(role, environment), entry], { env: { ...process.env, ...environment }, stdio: ['ignore', output, output] });
		} finally {
			closeSync(output);
		}
	};
	if (process.platform === 'win32') {
		// Windows asks no permission to capture; Desktop Duplication needs the
		// signed-in desktop, which a process started over SSH is not in.
		const { target } = defaultNativeOptions();
		return {
			readerCrate: 'vnilink-windows',
			reader: join(root, 'target', ...(target ? [target] : []), 'release/examples/glass_latency.exe'),
			consentMs: 0,
			launch: direct(() => []),
		};
	}
	if (process.platform === 'linux') {
		// The portal asks someone at this computer twice, for the host's
		// screen and for the reader's, and needs the graphical session's
		// bus. The sides run under XWayland: a Wayland client cannot place
		// the stimulus at the top of the display or the viewer below it.
		// Across two computers the viewer reads no screen, so it needs no
		// placed window and runs as a Wayland client: Chromium decodes in
		// hardware through VA-API there, which is what a viewer measured
		// across two computers should show. Under XWayland it decodes in
		// software.
		return {
			readerCrate: 'vnilink-linux',
			reader: join(root, 'target/release/examples/glass_latency'),
			consentMs: 5 * 60000,
			launch: direct((role, environment) => [role === 'viewer' && environment.VNILINK_LATENCY_ACROSS === '1' ? '--ozone-platform=wayland' : '--ozone-platform=x11']),
		};
	}
	throw new Error('measure:latency reads a macOS, Windows or Linux display');
}

/**
 * Carries each side's rendezvous files to the other through the agent
 * until `stopped`: both sides write a file once, whole, by renaming, so a
 * file is carried once. `running` hears whether the agent's side still runs.
 */
async function relayRendezvous(
	session: AgentSession, here: string, there: 'host' | 'viewer', stopped: () => boolean, running: (running: boolean) => void, signal: AbortSignal,
): Promise<void> {
	const carried = new Set<string>();
	const taken = agentSideTakes[there].map(name => `${name}.json`);
	while (!stopped()) {
		signal.throwIfAborted();
		for (const name of (await readdir(here)).filter(name => taken.includes(name) && !carried.has(name))) {
			await session.put(name, await readFile(join(here, name)));
			carried.add(name);
		}
		const polled = await session.poll();
		const strange = polled.files.find(file => !rendezvousFile(file.name));
		if (strange) {
			throw new Error(`The agent returned a file that is no rendezvous file: ${JSON.stringify(strange.name.slice(0, 80))}`);
		}
		for (const file of polled.files.filter(file => !carried.has(file.name))) {
			await writeFile(join(here, `${file.name}.partial`), file.data);
			await rename(join(here, `${file.name}.partial`), join(here, file.name));
			carried.add(file.name);
		}
		running(polled.running);
		await new Promise(resolve => setTimeout(resolve, 250));
	}
}

/** The variables a latency side on the agent may be given: `LATENCY_VARIABLES` in the test kit's `src/peer/scenarios/launch.rs`. */
export const agentLatencyVariables = {
	VNILINK_LATENCY_FPS: [0, 240],
	VNILINK_LATENCY_BITRATE: [0, 50_000],
	VNILINK_LATENCY_BUSY: [0, 1],
	VNILINK_LATENCY_CONSENT_MS: [0, 600_000],
} as const;

/** Whether `name` is a rendezvous file's: as the agent names them, so a file it returns lands only in the rendezvous folder. */
export function rendezvousFile(name: string): boolean {
	return /^[A-Za-z0-9_-]{1,64}\.json$/.test(name) && !/^(con|prn|aux|nul|com[1-9]|lpt[1-9])\./i.test(name);
}

/**
 * The rendezvous files a side on the agent takes, by role: what the other
 * role and the runner publish (`accepted` in the test kit's
 * `src/peer/scenarios/launch.rs`, which also checks what each holds).
 */
export const agentSideTakes = {
	host: ['viewer', 'viewer-streaming', 'read', 'done'],
	viewer: ['host', 'host-ready', 'read', 'read-done', 'done'],
} as const;

/** The key an invitation token (`vnl1.` and hex) was made by, as the agent reads it. */
export function invitationKey(token: unknown): string | undefined {
	const key = typeof token === 'string' && token.startsWith('vnl1.') ? token.slice(5, 69).toLowerCase() : '';
	return /^[0-9a-f]{64}$/.test(key) ? key : undefined;
}

/**
 * One side on this computer and the other on the test agent's computer,
 * the pair a person uses: each side's own stage times, the host's capture
 * and encode and the viewer's queue, decode and drawing, with the round
 * trip for the network. No screen shows both, so there is no
 * glass-to-glass total.
 */
async function measureAcross(root: string, signal: AbortSignal, options: LatencyOptions, peer: LatencyPeer): Promise<void> {
	await assertNativeFresh(root);
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vl-'));
	const here: 'host' | 'viewer' = peer.hosts === 'here' ? 'host' : 'viewer';
	const there: 'host' | 'viewer' = here === 'host' ? 'viewer' : 'host';
	const rendezvous = join(work, 'rendezvous');
	let session: AgentSession | undefined;
	let started = false;
	// Changed by the sides and the relay while this function waits.
	const state = { stopped: false, localExited: false, peerRunning: true };
	const sides: Promise<unknown>[] = [];
	let fetched = false;
	/** The agent's side's evidence joins this side's, once, for a run that failed too. */
	const fetchEvidence = async (): Promise<void> => {
		if (fetched || !session || !started) {
			return;
		}
		fetched = true;
		await session.fetch(options.output);
	};
	try {
		await mkdir(options.output, { recursive: true });
		await mkdir(rendezvous);
		session = await openAgentSession(root, peer, signal);
		const grants = join(root, '.build/latency');
		await mkdir(grants, { recursive: true, mode: 0o700 });
		// A Linux host's portal asks someone there; the other side waits as long.
		const hostOs = here === 'host' ? osName : session.platform.os;
		const shared: Record<keyof typeof agentLatencyVariables, string> = {
			VNILINK_LATENCY_FPS: String(options.frameRate ?? 0),
			VNILINK_LATENCY_BITRATE: String(options.bitrate ?? 0),
			VNILINK_LATENCY_BUSY: options.busy ? '1' : '0',
			VNILINK_LATENCY_CONSENT_MS: String(hostOs === 'linux' ? 5 * 60000 : 0),
		};
		const platform = await latencyPlatform(root, options.app, processes, signal);
		const profile = join(work, here);
		await mkdir(profile);
		const local = platform.launch(here, {
			...shared,
			// Each side on its own computer, as a person uses them; the agent
			// sets it for its side itself.
			VNILINK_LATENCY_ACROSS: '1',
			VNILINK_LATENCY_ROLE: here,
			VNILINK_LATENCY_DIR: rendezvous,
			VNILINK_LATENCY_OUTPUT: options.output,
			VNILINK_LATENCY_SCREEN_GRANT: join(grants, 'host-screen-grant'),
			VNILINK_TEST_PROFILE: profile,
		}, join(options.output, `${here}.log`));
		sides.push(local.result.catch(() => undefined).finally(() => {
			state.localExited = true;
		}));
		// The agent starts the other side from its own checkout, with its own
		// folders and grant; this side names only the scenario and the numbers,
		// and, for a host there, the one viewer it may show its screen to.
		const params: Record<string, unknown> = { env: shared };
		if (there === 'host') {
			await arrival(rendezvous, 'viewer', 120000, () => state.localExited, signal);
			const viewerKey = invitationKey((JSON.parse(await readFile(join(rendezvous, 'viewer.json'), 'utf8')) as Record<string, unknown>).token);
			if (!viewerKey) {
				throw new Error('The viewer here published no invitation');
			}
			params.viewer_key = viewerKey;
		}
		await session.start(`latency-${there}`, params);
		started = true;
		const relaying = relayRendezvous(session, rendezvous, there, () => state.stopped, running => {
			state.peerRunning = running;
		}, signal).catch((error: unknown) => {
			state.peerRunning = false;
			throw error;
		});
		relaying.catch(() => undefined);
		const exited = (): boolean => state.localExited || !state.peerRunning;
		// The portal on a Linux host asks someone there once; its answer is kept.
		await arrival(rendezvous, 'viewer-streaming', 120000 + 5 * 60000, exited, signal);
		await new Promise(resolve => setTimeout(resolve, options.settleSeconds * 1000));
		await publish(rendezvous, 'read', { seconds: options.seconds });
		await arrival(rendezvous, 'read-done', (options.seconds + 60) * 1000, exited, signal);
		await arrival(rendezvous, 'viewer-cpu', 5000, () => false, signal).catch(() => undefined);
		await publish(rendezvous, 'done', {});
		// Both sides quit on `done`, the agent's once the relay carries it.
		const quit = Date.now() + 30000;
		while ((!state.localExited || state.peerRunning) && Date.now() < quit) {
			await new Promise(resolve => setTimeout(resolve, 250));
		}
		state.stopped = true;
		await relaying.catch(() => undefined);
		await fetchEvidence();
		const read = async (name: string): Promise<string> => readFile(join(options.output, name), 'utf8').catch(() => '');
		const cpuOf = async (name: string): Promise<Record<string, number>> => {
			const value: unknown = JSON.parse(await readFile(join(rendezvous, `${name}.json`), 'utf8').catch(() => '{}'));
			return typeof value === 'object' && value !== null ? Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number')) : {};
		};
		const stages = await read('stages.jsonl');
		const summary = renderAcrossSummary(
			summarisePipeline(await read('host-stages.jsonl')),
			summarisePipeline(await read('viewer-pipeline.jsonl')),
			summariseLatency('', stages),
			summariseViewerStatus(stages),
			{ host: await cpuOf('host-cpu'), viewer: await cpuOf('viewer-cpu') },
		);
		// Each computer by what it runs, never by its name.
		const agent = `${session.platform.os} ${session.platform.arch} (the agent)`;
		const run = {
			seconds: options.seconds, settle: options.settleSeconds, fps: options.frameRate ?? null, bitrate: options.bitrate ?? null, busy: options.busy,
			host: here === 'host' ? `${osName} ${process.arch} (here)` : agent,
			viewer: here === 'viewer' ? `${osName} ${process.arch} (here)` : agent,
			date: new Date().toISOString(),
		};
		await writeFile(join(options.output, 'run.json'), JSON.stringify(run, null, 2));
		await writeFile(join(options.output, 'summary.md'), summary);
		console.log(summary);
		console.log(`Evidence: ${options.output}`);
	} finally {
		state.stopped = true;
		await publish(rendezvous, 'done', {}).catch(() => undefined);
		await settledWithin(Promise.all(sides), 20000);
		await fetchEvidence().catch(() => undefined);
		// The agent tells its side `done`, ends it if it does not quit, and
		// removes its folders; its checkout keeps only the portal's grant.
		if (started) {
			await session?.stop().catch(() => undefined);
		}
		await session?.close();
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}

/** The environment a latency side on the agent's computer runs with: what the agent set, and the suite's whitelisted numbers. */
export function agentLatencyEnvironment(role: 'host' | 'viewer', environment: NodeJS.ProcessEnv): Record<string, string> {
	// Only the agent starts a side here, and its other half is always on another computer.
	const result: Record<string, string> = { VNILINK_LATENCY_ROLE: role, VNILINK_LATENCY_ACROSS: '1' };
	for (const name of ['VNILINK_LATENCY_DIR', 'VNILINK_LATENCY_OUTPUT', 'VNILINK_LATENCY_SCREEN_GRANT', 'VNILINK_TEST_PROFILE']) {
		const value = environment[name];
		if (!value) {
			throw new Error(`The agent sets ${name} for a latency side`);
		}
		result[name] = value;
	}
	for (const [name, [least, most]] of Object.entries(agentLatencyVariables)) {
		const value = environment[name];
		if (value !== undefined && /^\d{1,9}$/.test(value) && Number(value) >= least && Number(value) <= most) {
			result[name] = value;
		}
	}
	return result;
}

/**
 * One side of a two-computer latency run on the test agent's computer
 * (`testkit:launch latency-host|latency-viewer`), started as a run on one
 * computer starts it, until `stopped` settles; the side then quits by
 * itself on the `done` the agent publishes, or is ended.
 */
export async function launchLatencySide(root: string, signal: AbortSignal, role: 'host' | 'viewer', stopped: Promise<void>): Promise<void> {
	await assertNativeFresh(root);
	await assertBundleFresh(root, 'latency.cjs');
	const environment = agentLatencyEnvironment(role, process.env);
	const processes = new ProcessRunner(signal);
	try {
		const platform = await latencyPlatform(root, undefined, processes, signal);
		const side = platform.launch(role, environment, join(environment.VNILINK_LATENCY_OUTPUT, `${role}.log`));
		await Promise.race([side.result, stopped]);
		await settledWithin(side.result, 20000);
	} finally {
		await processes.stop();
	}
}

export async function measureLatency(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const options = latencyOptions(root, args);
	if (options.peer) {
		await measureAcross(root, signal, options, options.peer);
		return;
	}
	await assertNativeFresh(root);
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vnilink-latency-'));
	const sides: Promise<unknown>[] = [];
	try {
		await mkdir(options.output, { recursive: true });
		const platform = await latencyPlatform(root, options.app, processes, signal);
		// The reader is built before the sides start, so its compile does not
		// count against their timeouts.
		await processes.run('cargo', ['build', '--release', '--locked', '-p', platform.readerCrate, '--example', 'glass_latency'], { cwd: root });
		const rendezvous = join(work, 'rendezvous');
		await mkdir(rendezvous);
		// Where the desktop portal's grants are kept between runs, for the
		// host's screen and for the reader's, so each is asked for once.
		const grants = join(root, '.build/latency');
		await mkdir(grants, { recursive: true, mode: 0o700 });
		let exited = false;
		for (const role of ['host', 'viewer'] as const) {
			const profile = join(work, role);
			await mkdir(profile);
			const environment = {
				VNILINK_TEST_PROFILE: profile,
				VNILINK_LATENCY_ROLE: role,
				VNILINK_LATENCY_DIR: rendezvous,
				VNILINK_LATENCY_OUTPUT: options.output,
				VNILINK_LATENCY_HOLD_MS: String(options.holdSeconds * 1000),
				VNILINK_LATENCY_FPS: String(options.frameRate ?? 0),
				VNILINK_LATENCY_BITRATE: String(options.bitrate ?? 0),
				VNILINK_LATENCY_BUSY: options.busy ? '1' : '0',
				VNILINK_LATENCY_SCREEN_GRANT: join(grants, 'host-screen-grant'),
			};
			const side = platform.launch(role, environment, join(options.output, `${role}.log`));
			sides.push(side.result.catch(() => undefined).finally(() => {
				exited = true;
			}));
		}
		await arrival(rendezvous, 'viewer-streaming', 120000 + platform.consentMs, () => exited, signal).catch(async (error: unknown) => {
			// Each side says why in its log.
			const logs = await Promise.all(['host', 'viewer'].map(async role => `${role}:\n${(await readFile(join(options.output, `${role}.log`), 'utf8').catch(() => '')).slice(-1500)}`));
			throw new Error(`${error instanceof Error ? error.message : String(error)}\n${logs.join('\n')}`);
		});
		await new Promise(resolve => setTimeout(resolve, options.settleSeconds * 1000));
		const samples = join(options.output, 'samples.jsonl');
		// The host runs the reader, under its own Screen Recording permission.
		await publish(rendezvous, 'read', { reader: platform.reader, seconds: options.seconds, samples, grant: join(grants, 'reader-screen-grant') });
		await arrival(rendezvous, 'read-done', (options.seconds + 60) * 1000 + platform.consentMs, () => exited, signal);
		const read: unknown = JSON.parse(await readFile(join(rendezvous, 'read-done.json'), 'utf8'));
		if (typeof read !== 'object' || read === null || !('code' in read) || read.code !== 0) {
			throw new Error(`The reader failed: ${JSON.stringify(read)}`);
		}
		const run = {
			seconds: options.seconds, settle: options.settleSeconds, hold: options.holdSeconds,
			platform: process.platform, arch: process.arch, date: new Date().toISOString(),
		};
		await writeFile(join(options.output, 'run.json'), JSON.stringify(run, null, 2));
		// The viewer samples once a second: let it report before it is told to quit.
		await arrival(rendezvous, 'viewer-cpu', 5000, () => false, signal).catch(() => undefined);
		await publish(rendezvous, 'done', {});
		const stages = await readFile(join(options.output, 'stages.jsonl'), 'utf8').catch(() => '');
		const cpuOf = async (name: string): Promise<Record<string, number>> => {
			const value: unknown = JSON.parse(await readFile(join(rendezvous, `${name}.json`), 'utf8').catch(() => '{}'));
			return typeof value === 'object' && value !== null ? Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number')) : {};
		};
		const cpu = { host: await cpuOf('host-cpu'), viewer: await cpuOf('viewer-cpu') };
		await writeFile(join(options.output, 'cpu.json'), JSON.stringify(cpu, null, 2));
		const pipeline: unknown = JSON.parse(await readFile(join(rendezvous, 'host-pipeline.json'), 'utf8').catch(() => '{}'));
		await writeFile(join(options.output, 'host-pipeline.json'), JSON.stringify(pipeline, null, 2));
		const encoder = latencyEncoder(pipeline);
		let summary = renderLatencySummary(summariseLatency(await readFile(samples, 'utf8'), stages), cpu, encoder);
		if (options.holdSeconds > 0) {
			const schedule = JSON.parse(await readFile(join(options.output, 'schedule.json'), 'utf8')) as HoldSchedule;
			const stills = await readFile(join(options.output, 'stills.jsonl'), 'utf8').catch(() => '');
			summary += `\n${renderHoldSummary(summariseHolds(schedule, stills, await readFile(samples, 'utf8')))}`;
		}
		await writeFile(join(options.output, 'summary.md'), summary);
		console.log(summary);
		console.log(`Evidence: ${options.output}`);
	} finally {
		// The sides quit on `done`; stopping `open` would not stop them.
		await publish(join(work, 'rendezvous'), 'done', {}).catch(() => undefined);
		// A side's process (`open -W` on macOS) returns when it has quit; its
		// profile is then free to remove.
		await settledWithin(Promise.all(sides), 20000);
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}
