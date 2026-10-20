/**
 * Impaired-network measurement: two VniLink engines in separate network
 * namespaces, joined by a `tc netem` veth pair, run the transfer matrix and
 * the desktop stream under each selected profile.
 *
 * This is Linux protocol and engine evidence only. It says nothing about
 * macOS or Windows capture, encoding, scheduling or network stacks.
 */
import { execFile } from 'node:child_process';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { applyProfile, createTopology, disposeTopology, linkStatistics, selectProfiles, statisticsDelta, topology, writeBurstHooks, type LinkCounters, type NetemProfile, type Topology } from './netem.ts';
import { ProcessRunner } from './processes.ts';

export const SCENARIOS = Object.freeze({
	transfer: Object.freeze({ crate: 'vnilink', test: 'tests::impaired_transfer_tests::impaired_transfer_matrix' }),
	video: Object.freeze({ crate: 'vnilink-media', test: 'tests::impaired_video_tests::impaired_desktop_stream' }),
});

export type ScenarioName = keyof typeof SCENARIOS;

export interface ImpairedOptions {
	readonly profiles: readonly string[];
	readonly scenarios: readonly string[];
	readonly seed: number;
	readonly smallFiles: number;
	readonly smallBytes: number;
	readonly largeBytes: number;
	readonly transferTimeoutSeconds: number;
	readonly driver: string;
	readonly videoSeconds: number;
	readonly bursts: number;
	readonly burstMs: number;
	readonly viewer: string;
}

export interface ResolvedOptions extends Omit<ImpairedOptions, 'profiles' | 'scenarios'> {
	readonly profiles: readonly NetemProfile[];
	readonly scenarios: readonly ScenarioName[];
}

export const DEFAULT_OPTIONS: ImpairedOptions = Object.freeze({
	profiles: ['all'],
	scenarios: ['transfer', 'video'],
	seed: 17,
	smallFiles: 1000,
	smallBytes: 4096,
	// CI-sized; `--large-size 10GiB` measures the long-transfer case.
	largeBytes: 1024 ** 3,
	transferTimeoutSeconds: 120,
	driver: 'tight',
	videoSeconds: 30,
	bursts: 5,
	burstMs: 250,
	viewer: 'passive',
});

const SIZE = /^(\d+)(k|m|g)?i?b?$/i;

/** Parses `4096`, `256MiB` or `10G` as a byte count. */
export function parseSize(text: string): number {
	const match = SIZE.exec(text.trim());
	if (!match) {
		throw new Error(`Not a size: ${text}`);
	}
	const units: Record<string, number> = { k: 1024, m: 1024 ** 2, g: 1024 ** 3 };
	const suffix = match.at(2)?.toLowerCase();
	return Number(match[1]) * (suffix ? units[suffix] : 1);
}

function whole(name: string, text: string | undefined): number | undefined {
	if (text === undefined) {
		return undefined;
	}
	if (!/^\d+$/.test(text)) {
		throw new Error(`--${name} must be a whole number`);
	}
	return Number(text);
}

/** Reads `--profiles loss-1,quick --large-size 1GiB ...` into options; unknown flags are rejected. */
export function parseImpairedArguments(args: readonly string[]): Partial<ImpairedOptions> & { output?: string } {
	const { values } = parseArgs({
		args: [...args],
		options: {
			'output': { type: 'string' },
			'profiles': { type: 'string' },
			'scenarios': { type: 'string' },
			'seed': { type: 'string' },
			'small-files': { type: 'string' },
			'small-size': { type: 'string' },
			'large-size': { type: 'string' },
			'transfer-timeout': { type: 'string' },
			'driver': { type: 'string' },
			'video-seconds': { type: 'string' },
			'bursts': { type: 'string' },
			'burst-ms': { type: 'string' },
			'viewer': { type: 'string' },
		},
		strict: true,
		allowPositionals: false,
	});
	const list = (text: string | undefined) => text?.split(',').map(item => item.trim()).filter(Boolean);
	const options = {
		output: values.output,
		profiles: list(values.profiles),
		scenarios: list(values.scenarios),
		seed: whole('seed', values.seed),
		smallFiles: whole('small-files', values['small-files']),
		smallBytes: values['small-size'] === undefined ? undefined : parseSize(values['small-size']),
		largeBytes: values['large-size'] === undefined ? undefined : parseSize(values['large-size']),
		transferTimeoutSeconds: whole('transfer-timeout', values['transfer-timeout']),
		driver: values.driver,
		videoSeconds: whole('video-seconds', values['video-seconds']),
		bursts: whole('bursts', values.bursts),
		burstMs: whole('burst-ms', values['burst-ms']),
		viewer: values.viewer,
	};
	return Object.fromEntries(Object.entries(options).filter(([, value]) => value !== undefined));
}

/** Validates options and resolves profile and scenario names. */
export function resolveOptions(options: Partial<ImpairedOptions>): ResolvedOptions {
	const resolved = { ...DEFAULT_OPTIONS, ...options };
	const scenarios = resolved.scenarios.map(name => {
		if (!Object.hasOwn(SCENARIOS, name)) {
			throw new Error(`Unknown scenario: ${name}. Known scenarios: ${Object.keys(SCENARIOS).join(', ')}`);
		}
		return name as ScenarioName;
	});
	if (!['tight', 'background'].includes(resolved.driver)) {
		throw new Error('The transfer driver must be tight or background');
	}
	if (!['passive', 'request-while-corrupt'].includes(resolved.viewer)) {
		throw new Error('The viewer policy must be passive or request-while-corrupt');
	}
	for (const key of ['seed', 'smallFiles', 'smallBytes', 'largeBytes', 'transferTimeoutSeconds', 'videoSeconds', 'bursts', 'burstMs'] as const) {
		if (!Number.isSafeInteger(resolved[key]) || resolved[key] < 0) {
			throw new Error(`${key} must be a whole number`);
		}
	}
	return { ...resolved, profiles: selectProfiles(resolved.profiles), scenarios };
}

export interface ScenarioPaths {
	readonly result: string;
	readonly scratch: string;
	readonly burstStart: string;
	readonly burstEnd: string;
}

/** Environment for one scenario run; the Rust scenarios read only these. */
export function scenarioEnvironment(name: ScenarioName, options: ResolvedOptions, paths: ScenarioPaths, link: Topology): Record<string, string> {
	const shared = {
		VNILINK_IMPAIRED_HOST_NS: link.names.host,
		VNILINK_IMPAIRED_VIEW_NS: link.names.view,
		VNILINK_IMPAIRED_OUT: paths.result,
		VNILINK_IMPAIRED_SCRATCH: paths.scratch,
	};
	if (name === 'transfer') {
		return {
			...shared,
			VNILINK_IMPAIRED_SMALL_FILES: String(options.smallFiles),
			VNILINK_IMPAIRED_SMALL_BYTES: String(options.smallBytes),
			VNILINK_IMPAIRED_LARGE_BYTES: String(options.largeBytes),
			VNILINK_IMPAIRED_TRANSFER_TIMEOUT_S: String(options.transferTimeoutSeconds),
			VNILINK_IMPAIRED_DRIVER: options.driver,
		};
	}
	return {
		...shared,
		VNILINK_IMPAIRED_VIDEO_SECONDS: String(options.videoSeconds),
		VNILINK_IMPAIRED_BURSTS: String(options.bursts),
		VNILINK_IMPAIRED_BURST_MS: String(options.burstMs),
		VNILINK_IMPAIRED_BURST_START: paths.burstStart,
		VNILINK_IMPAIRED_BURST_END: paths.burstEnd,
		VNILINK_IMPAIRED_VIEWER: options.viewer,
	};
}

interface TransferCase {
	case: string;
	outcome: string;
	error?: string | null;
	elapsed_ms?: number;
	mb_per_s?: number;
	files_per_s?: number;
}

interface VideoResult {
	delivery_ratio: number;
	clean_ratio: number;
	keyframes_produced: { open: number; forced: number; periodic: number };
	bursts: { time_to_picture_ms: number | null }[];
	unrecovered_bursts: number;
	latency_ms: { p50: number | null; p95: number | null };
	engine?: Record<'host' | 'viewer', { pipelines: Record<string, number>[] }>;
}

/** Engine counters: frames the viewer lost, keyframes it asked for, keyframes the host sent. */
function engineCounts(result: VideoResult): string {
	if (!result.engine) {
		return '–';
	}
	const total = (side: 'host' | 'viewer', keys: readonly string[]) => result.engine?.[side].pipelines.reduce((sum, pipeline) => sum + keys.reduce((inner, key) => inner + (pipeline[key] ?? 0), 0), 0) ?? 0;
	const lost = total('viewer', ['lost_expired', 'lost_evicted', 'lost_gap', 'lost_stale']);
	return `${String(lost)}/${String(total('viewer', ['keyframes_requested']))}/${String(total('host', ['keyframes_sent']))}`;
}

export interface ScenarioRun {
	profile: string;
	scenario: ScenarioName;
	status: 'passed' | 'failed';
	seconds: number;
	result: unknown;
	link: LinkCounters | null;
	error: string | null;
}

export interface ImpairedReport {
	started: string;
	commit: string | null;
	options?: unknown;
	profiles?: readonly NetemProfile[];
	runs: ScenarioRun[];
}

function cell(value: number | string | null | undefined, digits = 1): string {
	if (value === null || value === undefined) {
		return '–';
	}
	return typeof value === 'number' ? value.toFixed(digits) : value;
}

function median(values: readonly number[]): number | null {
	const sorted = values.toSorted((a, b) => a - b);
	return sorted.length ? sorted[Math.floor((sorted.length - 1) / 2)] : null;
}

/** A short Markdown summary of `report`. */
export function renderSummary(report: ImpairedReport): string {
	const lines = [
		'# Impaired-network run',
		'',
		`Commit ${report.commit ?? 'unknown'}, ${report.started}. Linux container evidence only: this does not cover macOS or Windows runtime behaviour.`,
		'',
	];
	const transfer = report.runs.filter(run => run.scenario === 'transfer');
	if (transfer.length) {
		lines.push('## Transfer (upload)', '', '| Profile | Case | Outcome | Elapsed s | MB/s | Files/s |', '|---|---|---|---|---|---|');
		for (const run of transfer) {
			const result = run.result as { cases: TransferCase[] } | null;
			if (!result) {
				lines.push(`| ${run.profile} | – | ${run.status} | – | – | – |`);
				continue;
			}
			for (const item of result.cases) {
				const outcome = item.outcome === 'refused' ? `refused: ${String(item.error)}` : item.outcome;
				lines.push(`| ${run.profile} | ${item.case} | ${outcome} | ${cell(item.elapsed_ms === undefined ? null : item.elapsed_ms / 1000)} | ${cell(item.mb_per_s, 2)} | ${cell(item.files_per_s)} |`);
			}
		}
		lines.push('');
	}
	const video = report.runs.filter(run => run.scenario === 'video');
	if (video.length) {
		const header = '| Profile | Delivered | Clean | Keyframes open/forced/periodic | Engine lost/requested/sent | Time to picture median/max ms | Unrecovered bursts | Latency p50/p95 ms |';
		lines.push('## Desktop video', '', header, '|---|---|---|---|---|---|---|---|');
		for (const run of video) {
			const result = run.result as VideoResult | null;
			if (!result) {
				lines.push(`| ${run.profile} | ${run.status} | – | – | – | – | – | – |`);
				continue;
			}
			const recovered = result.bursts.map(burst => burst.time_to_picture_ms).filter(value => value !== null);
			const keyframes = result.keyframes_produced;
			lines.push(`| ${run.profile} | ${cell(result.delivery_ratio * 100)}% | ${cell(result.clean_ratio * 100)}% | ${String(keyframes.open)}/${String(keyframes.forced)}/${String(keyframes.periodic)} | ${engineCounts(result)} | ${cell(median(recovered), 0)}/${cell(recovered.length ? Math.max(...recovered) : null, 0)} | ${String(result.unrecovered_bursts)}/${String(result.bursts.length)} | ${cell(result.latency_ms.p50)}/${cell(result.latency_ms.p95)} |`);
		}
		lines.push('');
	}
	const failed = report.runs.filter(run => run.status !== 'passed');
	if (failed.length) {
		lines.push('## Failed runs', '', ...failed.map(run => `- ${run.profile} ${run.scenario}: ${String(run.error)}`), '');
	}
	return lines.join('\n');
}

interface RunContext {
	readonly root: string;
	readonly signal: AbortSignal;
	readonly processes: ProcessRunner;
	readonly link: Topology;
	readonly options: ResolvedOptions;
	readonly output: string;
	readonly scratch: string;
	readonly targetDirectory: string;
}

async function runScenario(context: RunContext, item: NetemProfile, scenario: ScenarioName): Promise<ScenarioRun> {
	const { root, signal, processes, link, options, output, scratch, targetDirectory } = context;
	const directory = join(output, item.name);
	await mkdir(directory, { recursive: true });
	await mkdir(scratch, { recursive: true });
	const paths = {
		result: join(directory, `${scenario}.json`),
		scratch,
		burstStart: join(directory, 'burst-start.sh'),
		burstEnd: join(directory, 'burst-end.sh'),
	};
	await writeBurstHooks(link, item, options.seed, paths.burstStart, paths.burstEnd);
	const { crate, test } = SCENARIOS[scenario];
	const before = await linkStatistics(link, signal);
	const started = Date.now();
	const run: ScenarioRun = { profile: item.name, scenario, status: 'passed', seconds: 0, result: null, link: null, error: null };
	console.log(`[impaired] ${item.name} ${scenario}`);
	try {
		const args = ['test', '--release', '--locked', '--target-dir', targetDirectory, '-p', crate, '--lib', '--', '--ignored', '--exact', test, '--nocapture'];
		await processes.run('cargo', args, { cwd: root, env: { ...process.env, ...scenarioEnvironment(scenario, options, paths, link) } });
		run.result = JSON.parse(await readFile(paths.result, 'utf8')) as unknown;
	} catch (error) {
		signal.throwIfAborted();
		run.status = 'failed';
		run.error = error instanceof Error ? error.message : String(error);
	}
	run.seconds = (Date.now() - started) / 1000;
	run.link = statisticsDelta(before, await linkStatistics(link, signal));
	if (run.status === 'failed') {
		// A failed scenario may have left its blackout in place.
		await applyProfile(link, item, options.seed, signal);
	}
	return run;
}

/**
 * Builds the scenario tests once, then runs each selected profile and
 * scenario on a fresh link. Needs Linux and root; elsewhere use
 * `measure:impaired`, which runs this in the project container.
 */
export async function measureImpaired(root: string, signal: AbortSignal, args: readonly string[]): Promise<ImpairedReport> {
	if (process.platform !== 'linux') {
		throw new Error('The impaired-network harness needs Linux network namespaces; run `npm run measure:impaired` to use the container');
	}
	if (process.getuid?.() !== 0) {
		throw new Error('The impaired-network harness needs root to create network namespaces');
	}
	const { output: requested, ...parsed } = parseImpairedArguments(args);
	const options = resolveOptions(parsed);
	const output = requested ?? join(root, '.build/impaired-network', String(Date.now()));
	const processes = new ProcessRunner(signal);
	const context: RunContext = {
		root,
		signal,
		processes,
		link: topology(),
		options,
		output,
		// Fixture trees stay on local disk, outside the evidence directory.
		scratch: join(tmpdir(), `vnilink-impaired-${String(process.pid)}`),
		targetDirectory: process.env.CARGO_TARGET_DIR ?? join(root, 'target/impaired'),
	};
	const report: ImpairedReport = {
		started: new Date().toISOString(),
		commit: process.env.VNILINK_COMMIT ?? null,
		options: { ...options, profiles: options.profiles.map(item => item.name) },
		profiles: options.profiles,
		runs: [],
	};
	const crates = [...new Set(options.scenarios.map(name => SCENARIOS[name].crate))];
	const save = () => writeFile(join(output, 'results.json'), `${JSON.stringify(report, null, '\t')}\n`);
	await mkdir(output, { recursive: true });
	try {
		await processes.run('cargo', ['test', '--release', '--locked', '--no-run', '--target-dir', context.targetDirectory, ...crates.flatMap(name => ['-p', name]), '--lib'], { cwd: root });
		await createTopology(context.link, signal);
		for (const item of options.profiles) {
			await applyProfile(context.link, item, options.seed, signal);
			for (const scenario of options.scenarios) {
				report.runs.push(await runScenario(context, item, scenario));
				await save();
			}
		}
	} finally {
		await processes.stop();
		await disposeTopology(context.link);
		await rm(context.scratch, { recursive: true, force: true });
		await save();
		await writeFile(join(output, 'summary.md'), renderSummary(report));
		console.log(`Impaired-network evidence: ${output}`);
	}
	const failed = report.runs.filter(run => run.status !== 'passed');
	if (failed.length) {
		throw new Error(`${String(failed.length)} impaired-network run(s) failed; see ${join(output, 'summary.md')}`);
	}
	return report;
}

/** Runs `measureImpaired` in a privileged project container; arguments pass through. */
export async function measureImpairedInContainer(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	// Fail on bad arguments before building an image.
	resolveOptions(parseImpairedArguments(args));
	const output = join(root, '.build/impaired-network', String(Date.now()));
	const container = `vnilink-impaired-${String(process.pid)}-${String(Date.now())}`;
	const commit = await promisify(execFile)('git', ['rev-parse', 'HEAD'], { cwd: root }).then(result => result.stdout.trim(), () => 'unknown');
	const processes = new ProcessRunner(signal);
	let launched = false;
	await mkdir(output, { recursive: true });
	try {
		await processes.run('docker', ['info', '--format', '{{.OSType}} {{.Architecture}}']);
		await processes.run('docker', ['build', '-f', 'build/containers/Dockerfile.linux', '-t', 'vnilink-linux-validation:local', '.'], { cwd: root });
		launched = true;
		const environment = ['-e', `VNILINK_COMMIT=${commit}`, ...(process.env.CARGO_BUILD_JOBS ? ['-e', `CARGO_BUILD_JOBS=${process.env.CARGO_BUILD_JOBS}`] : [])];
		// Namespaces, veth devices and qdiscs need a privileged container.
		await processes.run('docker', [
			'run', '--name', container, '--privileged', '--init', ...environment,
			'--mount', `type=bind,source=${root},target=/source,readonly`,
			'--mount', `type=bind,source=${output},target=/evidence`,
			'--mount', 'type=volume,source=vnilink-linux-cargo-registry,target=/usr/local/cargo/registry',
			'--mount', 'type=volume,source=vnilink-impaired-release-target,target=/work/target',
			'--mount', 'type=volume,source=vnilink-impaired-node-modules,target=/work/node_modules',
			'--entrypoint', '/usr/local/bin/vnilink-impaired',
			'vnilink-linux-validation:local', ...args,
		]);
	} finally {
		await processes.stop();
		if (launched) {
			// Removal uses a separate owner because cancellation closes the work runner.
			const cleanup = new ProcessRunner();
			try {
				await cleanup.run('docker', ['rm', '--force', container]);
			} finally {
				await cleanup.stop();
			}
		}
		console.log(`Impaired-network evidence: ${output}`);
	}
}
