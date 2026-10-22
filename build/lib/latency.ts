/**
 * Glass-to-glass latency on one Mac or Windows computer: a host and a
 * viewer, each a temporary VniLink with its own identity, stream this
 * computer's screen to each other over loopback. The host shows a strip
 * carrying its frame time across the top of the main display; the viewer
 * shows the stream below it; a reader (ScreenCaptureKit on macOS, Desktop
 * Duplication on Windows) decodes both strips from the same picture.
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
}

export function latencyOptions(root: string, args: readonly string[]): LatencyOptions {
	const { values } = parseArgs({
		args: [...args],
		options: { seconds: { type: 'string', default: '20' }, settle: { type: 'string', default: '3' }, output: { type: 'string' }, app: { type: 'string' } },
		strict: true,
	});
	const seconds = Number(values.seconds);
	const settleSeconds = Number(values.settle);
	if (!Number.isInteger(seconds) || seconds < 1 || seconds > 600 || !Number.isFinite(settleSeconds) || settleSeconds < 0) {
		throw new Error('--seconds must be 1..600 and --settle at least 0');
	}
	if (values.app !== undefined && !/\.app\/?$/.test(values.app)) {
		throw new Error('--app must name a .app bundle');
	}
	// Absolute: the two sides run from another directory.
	const output = resolve(values.output ?? join(root, '.build/latency', new Date().toISOString().replaceAll(':', '-')));
	return { seconds, settleSeconds, output, app: values.app };
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
	if (process.platform === 'win32') {
		if (options.app !== undefined) {
			throw new Error('--app names a macOS application');
		}
		// Windows asks no permission to capture; Desktop Duplication needs the
		// signed-in desktop, which a process started over SSH is not in.
		const { target } = defaultNativeOptions();
		const electron = await electronExecutable(root, signal);
		return {
			readerCrate: 'vnilink-windows',
			reader: join(root, 'target', ...(target ? [target] : []), 'release/examples/glass_latency.exe'),
			launch: (_role, environment, log) => {
				const output = openSync(log, 'a');
				try {
					return processes.start(electron, [entry], { env: { ...process.env, ...environment }, stdio: ['ignore', output, output] });
				} finally {
					closeSync(output);
				}
			},
		};
	}
	throw new Error('measure:latency reads a macOS or Windows display');
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
		let exited = false;
		for (const role of ['host', 'viewer'] as const) {
			const profile = join(work, role);
			await mkdir(profile);
			const environment = {
				VNILINK_TEST_PROFILE: profile,
				VNILINK_LATENCY_ROLE: role,
				VNILINK_LATENCY_DIR: rendezvous,
				VNILINK_LATENCY_OUTPUT: options.output,
			};
			const side = platform.launch(role, environment, join(options.output, `${role}.log`));
			sides.push(side.result.catch(() => undefined).finally(() => {
				exited = true;
			}));
		}
		await arrival(rendezvous, 'viewer-streaming', 120000, () => exited, signal).catch(async (error: unknown) => {
			// Each side says why in its log.
			const logs = await Promise.all(['host', 'viewer'].map(async role => `${role}:\n${(await readFile(join(options.output, `${role}.log`), 'utf8').catch(() => '')).slice(-1500)}`));
			throw new Error(`${error instanceof Error ? error.message : String(error)}\n${logs.join('\n')}`);
		});
		await new Promise(resolve => setTimeout(resolve, options.settleSeconds * 1000));
		const samples = join(options.output, 'samples.jsonl');
		// The host runs the reader, under its own Screen Recording permission.
		await publish(rendezvous, 'read', { reader: platform.reader, seconds: options.seconds, samples });
		await arrival(rendezvous, 'read-done', (options.seconds + 60) * 1000, () => exited, signal);
		const read: unknown = JSON.parse(await readFile(join(rendezvous, 'read-done.json'), 'utf8'));
		if (typeof read !== 'object' || read === null || !('code' in read) || read.code !== 0) {
			throw new Error(`The reader failed: ${JSON.stringify(read)}`);
		}
		const run = { seconds: options.seconds, settle: options.settleSeconds, platform: process.platform, arch: process.arch, date: new Date().toISOString() };
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
		const summary = renderLatencySummary(summariseLatency(await readFile(samples, 'utf8'), stages), cpu, encoder);
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
