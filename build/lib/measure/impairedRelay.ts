// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The relay of the relayed path: its address, its counters and the sockets it carries, sampled while a scenario runs. */
import { appendFile } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import type { ProcessRunner } from '../processes.ts';
import { processOutput } from '../processes.ts';
import { RELAY_PATH, type Topology } from './netem.ts';

/** The local relay both cores use in relay mode; plain HTTP is what a core accepts for a loopback relay. */
export function relayUrl(): string {
	return `http://${RELAY_PATH.address}:${String(RELAY_PATH.port)}`;
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

/** The running relay's latest counters. */
export interface RelayMonitor {
	latest: RelayCounters | null;
}

/** Each relay TCP connection in `link`'s namespaces, as `ss` reports it now. */
async function relaySockets(link: Topology, signal: AbortSignal): Promise<RelaySocket[]> {
	const filter = `( sport = :${String(RELAY_PATH.port)} or dport = :${String(RELAY_PATH.port)} )`;
	const samples = await Promise.all(link.sides.map(async side => {
		const stdout = await processOutput('ip', ['netns', 'exec', side.namespace, 'ss', '-tinH', filter], { signal });
		return parseSocketSample(side.namespace, stdout);
	}));
	return samples.flat();
}

/**
 * Samples the relay's TCP connections each second until `done` settles, and
 * appends each sample to `log`. A failed sample is skipped: the record is
 * evidence beside the run, not part of its outcome.
 */
export async function sampleRelaySockets(link: Topology, signal: AbortSignal, log: string, done: Promise<unknown>): Promise<RelaySocket[]> {
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

/**
 * Starts the relay example in the host namespace and resolves once it
 * accepts connections. `processes` owns it: stopping the runner stops it.
 */
export async function startRelay(processes: ProcessRunner, binary: string, link: Topology): Promise<RelayMonitor> {
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
