// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** What each impaired-network scenario is given, and what its result must show for the run to count. */
import type { ResolvedOptions, ScenarioName } from './impairedOptions.ts';
import { relayUrl } from './impairedRelay.ts';
import type { LinkPath, Topology } from './netem.ts';

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
	// The transfer beside the stream is the large-file case, for as long as the stream runs.
	return name === 'combined' || name === 'combined-download' ? { ...video, VNILINK_IMPAIRED_LARGE_BYTES: String(options.largeBytes) } : video;
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

/** What the viewer's conditions must show for one profile; see `vnilink::Condition`. */
export interface ConditionExpectation {
	/** The condition held as worst most often; `null` for none. Unasserted when absent. */
	readonly dominant?: string | null;
	/** Conditions held at least once. */
	readonly seen: readonly string[];
}

/**
 * The plan's expectation per profile: loss profiles from 2% lose, a 2 Mb/s link
 * congests (held, not dominant: the stream starts at 4 Mb/s and leaves the
 * link within seconds), an unimpaired or merely delayed link shows nothing, a blackout
 * of seconds is one, and a relayed path says so. Profiles not named, such
 * as rate-5, are recorded but not asserted.
 */
export function expectedConditions(profile: string, path: LinkPath, burstMs: number, bursts: number): ConditionExpectation {
	const seen = [
		...path === 'relay' ? ['relay.path'] : [],
		...bursts > 0 && burstMs >= 2500 ? ['link.blackout'] : [],
	];
	// Blackouts of seconds fill much of the stream: they are asserted as
	// held, and the dominant condition is theirs, not the profile's.
	if (seen.includes('link.blackout')) {
		return { seen };
	}
	if (profile === 'clean' || profile.startsWith('delay-')) {
		return { dominant: null, seen };
	}
	// 0.5% and 1% random loss sit under the rate controller's own loss
	// threshold (2%), so the condition derived from it holds only briefly.
	if (profile === 'loss-0.5' || profile === 'loss-1') {
		return { seen };
	}
	// Relayed, the impaired leg is the relay's TCP connection: it repairs
	// the loss, so QUIC sees none while its round trip grows to seconds,
	// the relay backlog.
	if (profile.startsWith('loss-')) {
		return { dominant: path === 'relay' ? 'relay.backlog' : 'link.loss', seen };
	}
	if (profile === 'rate-2') {
		return { seen: [...seen, 'link.congestion'] };
	}
	return { seen };
}

/** Why a video scenario's conditions do not meet `expected`, or null when they do. */
export function conditionMismatch(result: unknown, expected: ConditionExpectation): string | null {
	const conditions = (result as { conditions?: { dominant?: unknown; seen?: unknown } } | null)?.conditions;
	if (!conditions || !Array.isArray(conditions.seen)) {
		return 'the scenario reported no conditions';
	}
	if (expected.dominant !== undefined && conditions.dominant !== expected.dominant) {
		const held = typeof conditions.dominant === 'string' ? conditions.dominant : 'none';
		return `the dominant condition was ${held}, not ${expected.dominant ?? 'none'}`;
	}
	const missing = expected.seen.filter(condition => !(conditions.seen as unknown[]).includes(condition));
	return missing.length ? `the viewer never held ${missing.join(', ')}` : null;
}
