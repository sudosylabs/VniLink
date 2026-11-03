// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The options of `measure:impaired` and their defaults (`impaired.ts` describes the run). */
import { parseArgs } from 'node:util';
import { selectProfiles, type LinkPath, type NetemProfile } from './netem.ts';

export const SCENARIOS = Object.freeze({
	transfer: Object.freeze({ crate: 'vnilink', test: 'core::jobs::impaired_transfer_tests::impaired_transfer_matrix' }),
	video: Object.freeze({ crate: 'vnilink-media', test: 'tests::impaired_video_tests::impaired_desktop_stream' }),
	/** The desktop stream while the viewer uploads one large file to the host. */
	combined: Object.freeze({ crate: 'vnilink-media', test: 'tests::impaired_video_tests::impaired_desktop_stream_during_upload' }),
	/** The desktop stream while the viewer downloads one large file from the host, the video's way. */
	'combined-download': Object.freeze({ crate: 'vnilink-media', test: 'tests::impaired_video_tests::impaired_desktop_stream_during_download' }),
	/** A screen that alternates still and moving, streamed without still pictures and then with them. */
	stills: Object.freeze({ crate: 'vnilink-media', test: 'tests::impaired_video_tests::impaired_desktop_stills' }),
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
