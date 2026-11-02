// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** What a latency run's samples say: percentiles, stage times, holds, and the summaries written from them. */

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
	// "50 fps · 76 ms · Direct"; "17 pictures replaced by newer ones before drawing"
	return { frameRate: read('status', /(\d+) fps/), roundTrip: read('status', /(\d+) ms\b/), replaced: read('text', /(\d+) pictures replaced/) };
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
export function latencyEncoder(pipeline: unknown): LatencyEncoder {
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
