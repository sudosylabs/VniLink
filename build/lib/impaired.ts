/**
 * Impaired-network measurement: two VniLink engines in separate network
 * namespaces, joined by a `tc netem` veth pair, run the transfer matrix, the
 * desktop stream and the stream beside a large upload under each selected
 * profile.
 *
 * This is Linux protocol and engine evidence only. It says nothing about
 * macOS or Windows capture, encoding, scheduling or network stacks.
 */
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFile, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { createInterface } from 'node:readline';
import { RELAY_PATH, applyProfile, createTopology, disposeTopology, linkStatistics, selectProfiles, statisticsDelta, topology, writeBurstHooks, type LinkCounters, type LinkPath, type NetemProfile, type Topology } from './netem.ts';
import { ProcessRunner } from './processes.ts';

export const SCENARIOS = Object.freeze({
	transfer: Object.freeze({ crate: 'vnilink', test: 'tests::impaired_transfer_tests::impaired_transfer_matrix' }),
	video: Object.freeze({ crate: 'vnilink-media', test: 'tests::impaired_video_tests::impaired_desktop_stream' }),
	/** The desktop stream while the viewer uploads one large file to the host. */
	combined: Object.freeze({ crate: 'vnilink-media', test: 'tests::impaired_video_tests::impaired_desktop_stream_during_upload' }),
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
	readonly path: string;
}

export interface ResolvedOptions extends Omit<ImpairedOptions, 'profiles' | 'scenarios' | 'path'> {
	readonly profiles: readonly NetemProfile[];
	readonly scenarios: readonly ScenarioName[];
	readonly path: LinkPath;
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
	path: 'direct',
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
			'path': { type: 'string' },
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
		path: values.path,
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
	if (resolved.path !== 'direct' && resolved.path !== 'relay') {
		throw new Error('The path must be direct or relay');
	}
	for (const key of ['seed', 'smallFiles', 'smallBytes', 'largeBytes', 'transferTimeoutSeconds', 'videoSeconds', 'bursts', 'burstMs'] as const) {
		if (!Number.isSafeInteger(resolved[key]) || resolved[key] < 0) {
			throw new Error(`${key} must be a whole number`);
		}
	}
	return { ...resolved, profiles: selectProfiles(resolved.profiles), scenarios, path: resolved.path };
}

/** The local relay both cores use in relay mode; plain HTTP is what a core accepts for a loopback relay. */
export function relayUrl(): string {
	return `http://${RELAY_PATH.address}:${String(RELAY_PATH.port)}`;
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
		...(options.path === 'relay' ? { VNILINK_IMPAIRED_RELAY_URL: relayUrl() } : {}),
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
	const video = {
		...shared,
		VNILINK_IMPAIRED_VIDEO_SECONDS: String(options.videoSeconds),
		VNILINK_IMPAIRED_BURSTS: String(options.bursts),
		VNILINK_IMPAIRED_BURST_MS: String(options.burstMs),
		VNILINK_IMPAIRED_BURST_START: paths.burstStart,
		VNILINK_IMPAIRED_BURST_END: paths.burstEnd,
		VNILINK_IMPAIRED_VIEWER: options.viewer,
	};
	// The upload is the large-file case, for as long as the stream runs.
	return name === 'combined' ? { ...video, VNILINK_IMPAIRED_LARGE_BYTES: String(options.largeBytes) } : video;
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
	/** The longest time without a new clean picture, after the first. */
	longest_freeze_ms?: number | null;
	/** How often the viewer's session left the active phase. */
	session_interruptions?: number;
	/** The combined scenario only. */
	upload?: { outcome: string; elapsed_ms: number; done_bytes: number; mb_per_s: number } | null;
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

type PathSample = Record<'host' | 'viewer', readonly { path: string }[]>;

/**
 * Why a scenario's reported paths do not show `expected`, or null when they
 * do. Each core must report a connection on `expected` when the scenario
 * finishes, and no sample may show a connection on the other path; a path
 * not yet selected (`none`) right after connecting is not a mismatch.
 */
export function pathMismatch(result: unknown, expected: LinkPath): string | null {
	const paths = (result as { paths?: { connected?: PathSample; finished?: PathSample } } | null)?.paths;
	if (!paths?.finished) {
		return 'the scenario reported no transport paths';
	}
	for (const [moment, sample] of [['connected', paths.connected], ['finished', paths.finished]] as const) {
		for (const side of ['host', 'viewer'] as const) {
			const seen = (sample?.[side] ?? []).map(connection => connection.path);
			const other = seen.find(path => path !== expected && path !== 'none');
			if (other) {
				return `the ${side} core's connection used the ${other} path when ${moment}, not ${expected}`;
			}
			if (moment === 'finished' && !seen.includes(expected)) {
				return `the ${side} core reported no ${expected} connection when finished (${seen.join(', ') || 'no connections'})`;
			}
		}
	}
	return null;
}

export interface ScenarioRun {
	profile: string;
	scenario: ScenarioName;
	status: 'passed' | 'failed';
	seconds: number;
	result: unknown;
	link: LinkCounters | null;
	/** Relay mode only: what the relay and its TCP connections did during the run. */
	relay?: RelayObservation | null;
	error: string | null;
}

/** The relay's cumulative counters, from its `metrics` lines (`examples/impaired_relay.rs`). */
export interface RelayCounters {
	bytes_recv: number;
	bytes_sent: number;
	frames_recv: number;
	frames_sent: number;
	frames_dropped: number;
	accepts: number;
	disconnects: number;
}

/** One TCP connection to or from the relay in one `ss -tin` sample. */
export interface RelaySocket {
	namespace: string;
	local: string;
	peer: string;
	/** Bytes the application wrote that the peer has not acknowledged. */
	sendQueue: number;
	rttMs: number | null;
	cwnd: number | null;
	/** Segments retransmitted over the connection's life. */
	retransmitted: number | null;
}

/** A scenario's relay counters and the peaks its relay connections reached. */
export interface RelayObservation {
	counters: RelayCounters | null;
	/**
	 * Datagram frames the relay took and neither wrote nor counted as
	 * dropped: what its per-client send queues discarded, give or take the
	 * frames still queued when the run ended.
	 */
	queueDiscarded: number | null;
	sockets: { namespace: string; local: string; peer: string; peakSendQueue: number; peakRttMs: number | null; retransmitted: number | null }[];
}

/** Parses `ss -tinH` output into one entry per connection. */
export function parseSocketSample(namespace: string, text: string): RelaySocket[] {
	const sockets: RelaySocket[] = [];
	for (const line of text.split('\n')) {
		const head = /^(\S+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(\S+)/.exec(line);
		if (head) {
			sockets.push({ namespace, local: head[4], peer: head[5], sendQueue: Number(head[3]), rttMs: null, cwnd: null, retransmitted: null });
			continue;
		}
		const socket = sockets.at(-1);
		if (socket && /^\s/.test(line)) {
			const rtt = /\brtt:([\d.]+)\//.exec(line);
			const cwnd = /\bcwnd:(\d+)/.exec(line);
			const retransmitted = /\bretrans:\d+\/(\d+)/.exec(line);
			socket.rttMs = rtt ? Number(rtt[1]) : socket.rttMs;
			socket.cwnd = cwnd ? Number(cwnd[1]) : socket.cwnd;
			socket.retransmitted = retransmitted ? Number(retransmitted[1]) : socket.retransmitted;
		}
	}
	return sockets;
}

/** Counter differences and per-connection peaks over one scenario. */
export function observeRelay(before: RelayCounters | null, after: RelayCounters | null, samples: readonly RelaySocket[]): RelayObservation {
	const difference = (start: RelayCounters, end: RelayCounters): RelayCounters => ({
		bytes_recv: end.bytes_recv - start.bytes_recv,
		bytes_sent: end.bytes_sent - start.bytes_sent,
		frames_recv: end.frames_recv - start.frames_recv,
		frames_sent: end.frames_sent - start.frames_sent,
		frames_dropped: end.frames_dropped - start.frames_dropped,
		accepts: end.accepts - start.accepts,
		disconnects: end.disconnects - start.disconnects,
	});
	const counters = before && after ? difference(before, after) : null;
	const peaks = new Map<string, RelayObservation['sockets'][number]>();
	for (const sample of samples) {
		const key = `${sample.namespace} ${sample.local} ${sample.peer}`;
		const { namespace, local, peer } = sample;
		const peak = peaks.get(key) ?? { namespace, local, peer, peakSendQueue: 0, peakRttMs: null, retransmitted: null };
		peak.peakSendQueue = Math.max(peak.peakSendQueue, sample.sendQueue);
		peak.peakRttMs = sample.rttMs === null ? peak.peakRttMs : Math.max(peak.peakRttMs ?? 0, sample.rttMs);
		peak.retransmitted = sample.retransmitted ?? peak.retransmitted;
		peaks.set(key, peak);
	}
	return {
		counters,
		queueDiscarded: counters ? counters.frames_recv - counters.frames_sent - counters.frames_dropped : null,
		sockets: [...peaks.values()],
	};
}

export interface ImpairedReport {
	started: string;
	commit: string | null;
	path?: LinkPath;
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

/** Time to picture median/max, unrecovered bursts and latency p50/p95, as summary cells. */
function videoCells(result: VideoResult): string[] {
	const recovered = result.bursts.map(burst => burst.time_to_picture_ms).filter(value => value !== null);
	return [
		`${cell(median(recovered), 0)}/${cell(recovered.length ? Math.max(...recovered) : null, 0)}`,
		`${String(result.unrecovered_bursts)}/${String(result.bursts.length)}`,
		`${cell(result.latency_ms.p50)}/${cell(result.latency_ms.p95)}`,
	];
}

/** A short Markdown summary of `report`. */
export function renderSummary(report: ImpairedReport): string {
	const lines = [
		'# Impaired-network run',
		'',
		`Commit ${report.commit ?? 'unknown'}, ${report.started}. Linux container evidence only: this does not cover macOS or Windows runtime behaviour.`,
		'',
	];
	if (report.path === 'relay') {
		lines.push('Path: relay. Direct UDP between the cores was blocked; traffic crossed a local iroh relay over the impaired link, not a public relay or an internet path.', '');
	}
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
			const keyframes = result.keyframes_produced;
			lines.push(`| ${run.profile} | ${cell(result.delivery_ratio * 100)}% | ${cell(result.clean_ratio * 100)}% | ${String(keyframes.open)}/${String(keyframes.forced)}/${String(keyframes.periodic)} | ${engineCounts(result)} | ${videoCells(result).join(' | ')} |`);
		}
		lines.push('');
	}
	const combined = report.runs.filter(run => run.scenario === 'combined');
	if (combined.length) {
		lines.push(
			'## Desktop video during an upload',
			'',
			'The viewer uploads one large file to the host while the host streams its desktop. The upload runs until it completes or the stream ends; MB/s counts the bytes it moved over that time. A freeze is the longest time without a new clean picture; an interruption is the viewer\'s session leaving the active phase.',
			'',
			'| Profile | Delivered | Time to picture median/max ms | Unrecovered bursts | Latency p50/p95 ms | Longest freeze ms | Session interruptions | Upload | Upload MB/s |',
			'|---|---|---|---|---|---|---|---|---|',
		);
		for (const run of combined) {
			const result = run.result as VideoResult | null;
			if (!result) {
				lines.push(`| ${run.profile} | ${run.status} | – | – | – | – | – | – | – |`);
				continue;
			}
			const upload = result.upload;
			const outcome = upload ? `${upload.outcome}, ${(upload.elapsed_ms / 1000).toFixed(1)} s` : '–';
			lines.push(`| ${run.profile} | ${cell(result.delivery_ratio * 100)}% | ${videoCells(result).join(' | ')} | ${cell(result.longest_freeze_ms, 0)} | ${cell(result.session_interruptions, 0)} | ${outcome} | ${cell(upload?.mb_per_s, 2)} |`);
		}
		lines.push('');
	}
	const relayed = report.runs.flatMap(run => run.relay ? [{ ...run, relay: run.relay }] : []);
	if (relayed.length) {
		lines.push(
			'## Relay',
			'',
			'Frames are datagram batches. Discarded frames are those the relay took and neither delivered nor dropped for a failed write: its per-client queues had no room. Send-Q is the largest unacknowledged backlog seen on a relay TCP connection of the viewer\'s leg, sampled each second.',
			'',
			'| Profile | Scenario | Frames in/out | Dropped/discarded | Client connections opened/closed | Viewer → relay Send-Q, RTT | Relay → viewer Send-Q, RTT |',
			'|---|---|---|---|---|---|---|',
		);
		for (const run of relayed) {
			const { counters, queueDiscarded, sockets } = run.relay;
			// A client that reconnects has several connections; the leg's peak is over all of them.
			const leg = (from: string) => {
				const own = sockets.filter(item => item.local.startsWith(`${from}:`) && [item.local, item.peer].some(end => end.startsWith(`${RELAY_PATH.viewerSource}:`)));
				if (!own.length) {
					return '–';
				}
				const sendQueue = Math.max(...own.map(item => item.peakSendQueue));
				const rtts = own.map(item => item.peakRttMs).filter(value => value !== null);
				return `${(sendQueue / 1e6).toFixed(2)} MB, ${cell(rtts.length ? Math.max(...rtts) : null, 0)} ms`;
			};
			const frames = counters ? `${String(counters.frames_recv)}/${String(counters.frames_sent)}` : '–';
			const lost = counters ? `${String(counters.frames_dropped)}/${String(queueDiscarded)}` : '–';
			const connections = counters ? `${String(counters.accepts)}/${String(counters.disconnects)}` : '–';
			lines.push(`| ${run.profile} | ${run.scenario} | ${frames} | ${lost} | ${connections} | ${leg(RELAY_PATH.viewerSource)} | ${leg(RELAY_PATH.address)} |`);
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
	/** Relay mode only. */
	relay?: RelayMonitor;
}

/** The running relay's latest counters. */
interface RelayMonitor {
	latest: RelayCounters | null;
}

/** Each relay TCP connection in `link`'s namespaces, as `ss` reports it now. */
async function relaySockets(link: Topology, signal: AbortSignal): Promise<RelaySocket[]> {
	const filter = `( sport = :${String(RELAY_PATH.port)} or dport = :${String(RELAY_PATH.port)} )`;
	const samples = await Promise.all(link.sides.map(async side => {
		const { stdout } = await promisify(execFile)('ip', ['netns', 'exec', side.namespace, 'ss', '-tinH', filter], { signal });
		return parseSocketSample(side.namespace, stdout);
	}));
	return samples.flat();
}

/**
 * Samples the relay's TCP connections each second until `done` settles, and
 * appends each sample to `log`. A failed sample is skipped: the record is
 * evidence beside the run, not part of its outcome.
 */
async function sampleRelaySockets(link: Topology, signal: AbortSignal, log: string, done: Promise<unknown>): Promise<RelaySocket[]> {
	const samples: RelaySocket[] = [];
	const state = { finished: false };
	void done.finally(() => {
		state.finished = true;
	});
	while (!state.finished && !signal.aborted) {
		const at = Date.now();
		const sockets = await relaySockets(link, signal).catch(() => []);
		samples.push(...sockets);
		await appendFile(log, sockets.map(socket => JSON.stringify({ at, ...socket })).join('\n') + (sockets.length ? '\n' : ''));
		await Promise.race([done, new Promise(resolve => setTimeout(resolve, 1_000))]).catch(() => undefined);
	}
	return samples;
}

async function runScenario(context: RunContext, item: NetemProfile, scenario: ScenarioName): Promise<ScenarioRun> {
	const { root, signal, processes, link, options, output, scratch, targetDirectory, relay } = context;
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
		const scenarioRun = processes.run('cargo', args, { cwd: root, env: { ...process.env, ...scenarioEnvironment(scenario, options, paths, link) } });
		if (relay) {
			const counters = relay.latest;
			const sockets = sampleRelaySockets(link, signal, join(directory, `${scenario}.relay-sockets.jsonl`), scenarioRun);
			try {
				await scenarioRun;
			} finally {
				const samples = await sockets;
				// The relay reports once a second; wait for a report made after the run.
				await new Promise(resolve => setTimeout(resolve, 1_100));
				run.relay = observeRelay(counters, relay.latest, samples);
			}
		} else {
			await scenarioRun;
		}
		run.result = JSON.parse(await readFile(paths.result, 'utf8')) as unknown;
		const mismatch = pathMismatch(run.result, options.path);
		if (mismatch) {
			throw new Error(`Wrong transport path: ${mismatch}`);
		}
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
 * Starts the relay example in the host namespace and resolves once it
 * accepts connections. `processes` owns it: stopping the runner stops it.
 */
async function startRelay(processes: ProcessRunner, binary: string, link: Topology): Promise<RelayMonitor> {
	const address = `${RELAY_PATH.address}:${String(RELAY_PATH.port)}`;
	const relay = processes.start('ip', ['netns', 'exec', link.names.host, binary, address], { stdio: ['ignore', 'pipe', 'inherit'] });
	const stdout = relay.child.stdout;
	if (!stdout) {
		throw new Error('The relay has no output to report readiness on');
	}
	const lines = createInterface({ input: stdout });
	// Keep draining its output so a full pipe never blocks it.
	const monitor: RelayMonitor = { latest: null };
	lines.on('line', line => {
		if (line.startsWith('metrics ')) {
			monitor.latest = JSON.parse(line.slice('metrics '.length)) as RelayCounters;
		} else {
			console.log(`[relay] ${line}`);
		}
	});
	let timer: NodeJS.Timeout | undefined;
	try {
		await Promise.race([
			new Promise<void>((resolve, reject) => {
				lines.on('line', line => {
					if (line.startsWith('listening ')) {
						resolve();
					}
				});
				lines.once('close', () => {
					reject(new Error('The relay exited before it listened'));
				});
			}),
			relay.result.then(() => {
				throw new Error('The relay exited before it listened');
			}),
			new Promise<never>((_, reject) => {
				timer = setTimeout(() => {
					reject(new Error('The relay did not listen within 30 seconds'));
				}, 30_000);
			}),
		]);
	} finally {
		clearTimeout(timer);
	}
	return monitor;
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
		path: options.path,
		options: { ...options, profiles: options.profiles.map(item => item.name) },
		profiles: options.profiles,
		runs: [],
	};
	const crates = [...new Set(options.scenarios.map(name => SCENARIOS[name].crate))];
	const save = () => writeFile(join(output, 'results.json'), `${JSON.stringify(report, null, '\t')}\n`);
	await mkdir(output, { recursive: true });
	try {
		await processes.run('cargo', ['test', '--release', '--locked', '--no-run', '--target-dir', context.targetDirectory, ...crates.flatMap(name => ['-p', name]), '--lib'], { cwd: root });
		if (options.path === 'relay') {
			await processes.run('cargo', ['build', '--release', '--locked', '--target-dir', context.targetDirectory, '-p', 'vnilink', '--example', 'impaired_relay'], { cwd: root });
		}
		await createTopology(context.link, signal, options.path);
		if (options.path === 'relay') {
			context.relay = await startRelay(processes, join(context.targetDirectory, 'release/examples/impaired_relay'), context.link);
		}
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

/**
 * A cache volume for one checkout: worktrees of the same repository build
 * different code, and a shared release target would let one run execute
 * another's binaries.
 */
export function checkoutVolume(name: string, root: string): string {
	return `${name}-${createHash('sha256').update(root).digest('hex').slice(0, 12)}`;
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
		// Runs on one machine compete for its CPUs, and both sets of numbers would be wrong.
		const running = await promisify(execFile)('docker', ['ps', '--filter', 'name=vnilink-impaired-', '--format', '{{.Names}}']).then(result => result.stdout.trim());
		if (running) {
			throw new Error(`Another impaired-network run is in progress (${running.split('\n').join(', ')}). Wait for it to finish: runs on one machine skew each other's numbers.`);
		}
		await processes.run('docker', ['build', '-f', 'build/containers/Dockerfile.linux', '-t', 'vnilink-linux-validation:local', '.'], { cwd: root });
		launched = true;
		const environment = ['-e', `VNILINK_COMMIT=${commit}`, ...(process.env.CARGO_BUILD_JOBS ? ['-e', `CARGO_BUILD_JOBS=${process.env.CARGO_BUILD_JOBS}`] : [])];
		// Namespaces, veth devices and qdiscs need a privileged container.
		await processes.run('docker', [
			'run', '--name', container, '--privileged', '--init', ...environment,
			'--mount', `type=bind,source=${root},target=/source,readonly`,
			'--mount', `type=bind,source=${output},target=/evidence`,
			'--mount', 'type=volume,source=vnilink-linux-cargo-registry,target=/usr/local/cargo/registry',
			'--mount', `type=volume,source=${checkoutVolume('vnilink-impaired-release-target', root)},target=/work/target`,
			'--mount', `type=volume,source=${checkoutVolume('vnilink-impaired-node-modules', root)},target=/work/node_modules`,
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
