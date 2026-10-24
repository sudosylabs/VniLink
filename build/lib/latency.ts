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
import { mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { closeSync, existsSync, openSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { electronExecutable } from './electron.ts';
import { assertNativeFresh, defaultNativeOptions } from './native.ts';
import { type LaunchedProcess, ProcessRunner } from './processes.ts';

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
}

export function latencyOptions(root: string, args: readonly string[]): LatencyOptions {
	const { values } = parseArgs({
		args: [...args],
		options: { seconds: { type: 'string', default: '20' }, settle: { type: 'string', default: '3' }, output: { type: 'string' }, app: { type: 'string' }, hold: { type: 'string', default: '0' } },
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
	// Absolute: the two sides run from another directory.
	const output = resolve(values.output ?? join(root, '.build/latency', new Date().toISOString().replaceAll(':', '-')));
	return { seconds, settleSeconds, output, app: values.app, holdSeconds };
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

async function latencyPlatform(root: string, options: LatencyOptions, processes: ProcessRunner, signal: AbortSignal): Promise<LatencyPlatform> {
	const entry = join(root, 'out/latency.cjs');
	if (process.platform === 'darwin') {
		// .../VniLink.app/Contents/MacOS/Electron
		const bundle = options.app ?? dirname(dirname(dirname(await electronExecutable(root, signal))));
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
	if (options.app !== undefined) {
		throw new Error('--app names a macOS application');
	}
	const electron = await electronExecutable(root, signal);
	const direct = (args: readonly string[]): LatencyPlatform['launch'] => (_role, environment, log) => {
		const output = openSync(log, 'a');
		try {
			return processes.start(electron, [...args, entry], { env: { ...process.env, ...environment }, stdio: ['ignore', output, output] });
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
			launch: direct([]),
		};
	}
	if (process.platform === 'linux') {
		// The portal asks someone at this computer twice, for the host's
		// screen and for the reader's, and needs the graphical session's
		// bus. The sides run under XWayland: a Wayland client cannot place
		// the stimulus at the top of the display or the viewer below it.
		return {
			readerCrate: 'vnilink-linux',
			reader: join(root, 'target/release/examples/glass_latency'),
			consentMs: 5 * 60000,
			launch: direct(['--ozone-platform=x11']),
		};
	}
	throw new Error('measure:latency reads a macOS, Windows or Linux display');
}

export async function measureLatency(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const options = latencyOptions(root, args);
	await assertNativeFresh(root);
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vnilink-latency-'));
	const sides: Promise<unknown>[] = [];
	try {
		await mkdir(options.output, { recursive: true });
		const platform = await latencyPlatform(root, options, processes, signal);
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
		await Promise.race([Promise.all(sides), new Promise(resolve => setTimeout(resolve, 20000))]);
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}
