// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The options of `measure:latency` (`latency.ts` describes the run):
 *
 *
 * - `--seconds` (20): how long the reader samples; `--settle` (3): seconds
 *   between the first picture and sampling; `--output`: where the evidence
 *   goes.
 * - `--app` (macOS): the `.app` that runs both sides; by default this
 *   checkout's development VniLink.
 * - `--hold` (0, off): hold mode, seconds the stimulus holds after each 2 s
 *   of motion.
 * - `--fps` and `--bitrate` (12000 kb/s; 50000 is Automatic): the viewer's
 *   frame rate at 1920x1080, and its bitrate limit with `--fps`.
 * - `--peer`, `--peer-check`, `--hosts` (`here`, or `peer`): the second
 *   computer, the digits its agent printed, and which one hosts its screen.
 * - `--busy` (with `--peer`): the stimulus fills the host's work area with
 *   detail that changes every frame, so the stream uses the bitrate it is
 *   given; without it the strip is nearly all that changes.
 */
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { agentPeer, type AgentPeer } from '../remote.ts';

export interface LatencyOptions {
	/** How long the reader samples, after the stream has settled. */
	readonly seconds: number;
	/** Settling time between the first picture and sampling. */
	readonly settleSeconds: number;
	readonly output: string;
	/**
	 * macOS: a development VniLink that already holds Screen Recording.
	 * macOS grants an ad-hoc signed copy by its code hash, so each copy needs
	 * its own grant. The application only hosts Electron; the code it runs is
	 * this checkout's `out/`.
	 */
	readonly app: string | undefined;
	/**
	 * Hold mode, in seconds; 0 when off. The stimulus moves for 2 s and
	 * holds for this long, over and over, above a panel of coloured text,
	 * so the host sends still pictures; the summary adds when each showed
	 * and what the first change after a hold took.
	 */
	readonly holdSeconds: number;
	/** The viewer's frame rate at the Smooth preset's bitrate and size; undefined keeps the default quality. */
	readonly frameRate: number | undefined;
	/** The viewer's bitrate limit in kb/s with `--fps`; undefined keeps the Smooth preset's 12 Mb/s. */
	readonly bitrate: number | undefined;
	/** The stimulus fills the screen below the strip with detail that changes every frame. */
	readonly busy: boolean;
	/** Two computers: the test agent on the other one; undefined runs both sides on this one. */
	readonly peer: LatencyPeer | undefined;
}

/** The other computer of a two-computer run: the test agent there, built from the same commit. */
export interface LatencyPeer extends AgentPeer {
	/** Which computer hosts its screen: this one, or the agent's. */
	readonly hosts: 'here' | 'peer';
}

export function latencyOptions(root: string, args: readonly string[]): LatencyOptions {
	const { values } = parseArgs({
		args: [...args],
		options: {
			seconds: { type: 'string', default: '20' }, settle: { type: 'string', default: '3' }, output: { type: 'string' }, app: { type: 'string' }, hold: { type: 'string', default: '0' },
			fps: { type: 'string' }, bitrate: { type: 'string' }, busy: { type: 'boolean', default: false }, peer: { type: 'string' }, 'peer-check': { type: 'string' }, hosts: { type: 'string', default: 'here' },
		},
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
	const frameRate = values.fps === undefined ? undefined : Number(values.fps);
	if (frameRate !== undefined && (!Number.isInteger(frameRate) || frameRate < 1 || frameRate > 120)) {
		throw new Error('--fps must be 1..120');
	}
	const bitrate = values.bitrate === undefined ? undefined : Number(values.bitrate);
	if (bitrate !== undefined && (!Number.isInteger(bitrate) || bitrate < 1 || bitrate > 50_000)) {
		throw new Error('--bitrate must be 1..50000 kb/s');
	}
	const agent = agentPeer(values.peer, values['peer-check']);
	if (values.busy && agent === undefined) {
		throw new Error('--busy needs --peer: the stimulus then fills the screen, and no viewer shares it');
	}
	if (values.hosts !== 'here' && values.hosts !== 'peer') {
		throw new Error('--hosts must be here or peer');
	}
	if (agent === undefined && values.hosts !== 'here') {
		throw new Error('--hosts needs --peer');
	}
	if (agent !== undefined && holdSeconds > 0) {
		throw new Error('--hold reads one screen and cannot run across two computers');
	}
	const peer = agent === undefined ? undefined : { ...agent, hosts: values.hosts === 'peer' ? 'peer' as const : 'here' as const };
	// Absolute: the two sides run from another directory.
	const output = resolve(values.output ?? join(root, '.build/latency', new Date().toISOString().replaceAll(':', '-')));
	return { seconds, settleSeconds, output, app: values.app, holdSeconds, frameRate, bitrate, busy: values.busy, peer };
}
