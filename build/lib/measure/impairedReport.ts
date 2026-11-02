// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** An impaired-network run's report and the summary written from it. */
import type { ScenarioName } from './impairedOptions.ts';
import type { RelayObservation } from './impairedRelay.ts';
import { RELAY_PATH, type LinkCounters, type LinkPath, type NetemProfile } from './netem.ts';

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
	/** The combined scenarios only. */
	transfer?: { direction: string; outcome: string; elapsed_ms: number; done_bytes: number; mb_per_s: number } | null;
	/** The viewer's conditions over the timeline: the worst held most often, and all held. */
	conditions?: { dominant: string | null; seen: string[] };
	/** Sampled every 100 ms; `rtt_ms` is the session connection's round trip. */
	timeline?: readonly { rtt_ms?: number | null }[];
}

/** The median of the session's round trip over the timeline, in ms. */
function sessionRtt(result: VideoResult): number | null {
	const samples = (result.timeline ?? []).flatMap(sample => typeof sample.rtt_ms === 'number' ? [sample.rtt_ms] : []).sort((a, b) => a - b);
	return samples.length ? samples[Math.floor(samples.length / 2)] : null;
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
	/** Relay mode only: what the relay and its TCP connections did during the run. */
	relay?: RelayObservation | null;
	error: string | null;
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

/** One half of a stills run: without still pictures or with them. */
interface StillsRun {
	changes: number;
	changes_unshown: number;
	first_picture_latency_ms: { p50: number | null; p95: number | null; max: number | null };
	latency_ms: { p50: number | null; p95: number | null };
	stills_received: number;
	still_after_ms?: { p50: number | null; max: number | null };
	host: { stills_sent: number; still_bytes: number; stills_dropped: number } | null;
}

/** First changed picture's latency p50/p95/max, then all changed pictures' p95, as summary cells. */
function stillsCells(run: StillsRun): string[] {
	const first = run.first_picture_latency_ms;
	return [`${cell(first.p50)}/${cell(first.p95)}/${cell(first.max)}`, cell(run.latency_ms.p95), `${String(run.changes - run.changes_unshown)}/${String(run.changes)}`];
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
		const header = '| Profile | Delivered | Clean | Keyframes open/forced/periodic | Engine lost/requested/sent | Time to picture median/max ms | Unrecovered bursts | Latency p50/p95 ms | Dominant condition (held) |';
		lines.push('## Desktop video', '', header, '|---|---|---|---|---|---|---|---|---|');
		for (const run of video) {
			const result = run.result as VideoResult | null;
			if (!result) {
				lines.push(`| ${run.profile} | ${run.status} | – | – | – | – | – | – | – |`);
				continue;
			}
			const keyframes = result.keyframes_produced;
			const conditions = result.conditions ? `${result.conditions.dominant ?? 'none'} (${result.conditions.seen.join(', ') || 'none'})` : '–';
			lines.push(`| ${run.profile} | ${cell(result.delivery_ratio * 100)}% | ${cell(result.clean_ratio * 100)}% | ${String(keyframes.open)}/${String(keyframes.forced)}/${String(keyframes.periodic)} | ${engineCounts(result)} | ${videoCells(result).join(' | ')} | ${conditions} |`);
		}
		lines.push('');
	}
	const combined = report.runs.filter(run => run.scenario === 'combined' || run.scenario === 'combined-download');
	if (combined.length) {
		lines.push(
			'## Desktop video beside a large transfer',
			'',
			'The viewer uploads one large file to the host (`combined`) or downloads one from it (`combined-download`, the video\'s way) while the host streams its desktop. The transfer runs until it completes or the stream ends; MB/s counts the bytes it moved over that time. A freeze is the longest time without a new clean picture; an interruption is the viewer\'s session leaving the active phase; the session round trip is the median over the stream.',
			'',
			'| Profile | Direction | Delivered | Time to picture median/max ms | Unrecovered bursts | Latency p50/p95 ms | Longest freeze ms | Session interruptions | Session RTT ms | Transfer | Transfer MB/s |',
			'|---|---|---|---|---|---|---|---|---|---|---|',
		);
		for (const run of combined) {
			const direction = run.scenario === 'combined' ? 'upload' : 'download';
			const result = run.result as VideoResult | null;
			if (!result) {
				lines.push(`| ${run.profile} | ${direction} | ${run.status} | – | – | – | – | – | – | – | – |`);
				continue;
			}
			const transfer = result.transfer;
			const outcome = transfer ? `${transfer.outcome}, ${(transfer.elapsed_ms / 1000).toFixed(1)} s` : '–';
			lines.push(`| ${run.profile} | ${direction} | ${cell(result.delivery_ratio * 100)}% | ${videoCells(result).join(' | ')} | ${cell(result.longest_freeze_ms, 0)} | ${cell(result.session_interruptions, 0)} | ${cell(sessionRtt(result), 0)} | ${outcome} | ${cell(transfer?.mb_per_s, 2)} |`);
		}
		lines.push('');
	}
	const stills = report.runs.filter(run => run.scenario === 'stills');
	if (stills.length) {
		lines.push(
			'## Still pictures',
			'',
			'A screen of coloured text stays the same for 2.25 s, then moves for 0.75 s, over and over, at the Smooth preset (12 Mb/s): the largest still pictures, and each change lands while one is on the wire, so this is the longest a change can wait behind one. Each profile streams it without still pictures, then with them. The first picture of a change is the one a change waits for; its latency runs from when it was made to when it arrived. A still arrives this long after the screen went still.',
			'',
			'| Profile | First picture of a change, off p50/p95/max ms | On p50/p95/max ms | Every changed picture p95 off/on ms | Changes shown off/on | Stills sent/dropped/received | Still kB | Still arrives after, p50/max ms |',
			'|---|---|---|---|---|---|---|---|',
		);
		for (const run of stills) {
			const result = run.result as { off: StillsRun; on: StillsRun } | null;
			if (!result) {
				lines.push(`| ${run.profile} | ${run.status} | – | – | – | – | – | – |`);
				continue;
			}
			const [offFirst, offP95, offShown] = stillsCells(result.off);
			const [onFirst, onP95, onShown] = stillsCells(result.on);
			const host = result.on.host;
			const sent = host ? `${String(host.stills_sent)}/${String(host.stills_dropped)}/${String(result.on.stills_received)}` : '–';
			const size = host?.stills_sent ? cell(host.still_bytes / host.stills_sent / 1000, 0) : '–';
			const after = result.on.still_after_ms;
			lines.push(`| ${run.profile} | ${offFirst} | ${onFirst} | ${offP95}/${onP95} | ${offShown}/${onShown} | ${sent} | ${size} | ${cell(after?.p50, 0)}/${cell(after?.max, 0)} |`);
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
