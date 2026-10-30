// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Where a desktop picture's time goes, stage by stage:
 * - `host`: capture to send on the other computer;
 * - `queue`: queued on this computer before the window could take it;
 * - `bridge`: from the engine's process to the window;
 * - `decode`: in the decoder;
 * - `draw`: from decoded to drawn.
 * The network leg is not measured per picture: the clocks differ, so it is
 * read from the round trip instead.
 */
export const latencyStages = ['host', 'queue', 'bridge', 'decode', 'draw'] as const;

export type LatencyStage = typeof latencyStages[number];

/** Milliseconds, half of the stage's pictures at or below `p50`, nineteen in twenty at or below `p95`. */
export interface StageLatency {
	readonly p50: number;
	readonly p95: number;
	readonly count: number;
}

export type DesktopLatencySummary = Readonly<Partial<Record<LatencyStage, StageLatency>>>;

/** Pictures kept per stage: four seconds at 60 a second. */
const SAMPLES = 240;

/** The nearest-rank percentile of sorted samples. */
function percentile(sorted: readonly number[], fraction: number): number {
	return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(fraction * sorted.length) - 1))];
}

/** Collects stage times between reads; each read summarises and starts afresh. */
export class DesktopLatency {
	private samples = new Map<LatencyStage, number[]>();

	/** A stage time in milliseconds. A negative or non-finite time, as from clocks out of step, is left out. */
	public record(stage: LatencyStage, ms: number): void {
		if (!Number.isFinite(ms) || ms < 0) {
			return;
		}
		let samples = this.samples.get(stage);
		if (!samples) {
			samples = [];
			this.samples.set(stage, samples);
		}
		if (samples.length === SAMPLES) {
			samples.shift();
		}
		samples.push(ms);
	}

	/** Each measured stage since the last read. */
	public take(): DesktopLatencySummary {
		const summary: Partial<Record<LatencyStage, StageLatency>> = {};
		for (const [stage, samples] of this.samples) {
			const sorted = [...samples].sort((a, b) => a - b);
			summary[stage] = { p50: percentile(sorted, 0.5), p95: percentile(sorted, 0.95), count: sorted.length };
		}
		this.samples = new Map();
		return summary;
	}
}
