// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * What a latency side on the test agent may be given and send: the variables
 * a run names, the rendezvous files the sides exchange, and the side the agent
 * starts with them.
 */
import { join } from 'node:path';
import { assertBundleFresh } from '../bundles.ts';
import { assertNativeFresh } from '../native.ts';
import { ProcessRunner } from '../processes.ts';
import { latencyPlatform, settledWithin } from './latencySides.ts';

/** The variables a latency side on the agent may be given: `LATENCY_VARIABLES` in the test kit's `src/peer/scenarios/launch.rs`. */
export const agentLatencyVariables = {
	VNILINK_LATENCY_FPS: [0, 240],
	VNILINK_LATENCY_BITRATE: [0, 50_000],
	VNILINK_LATENCY_BUSY: [0, 1],
	VNILINK_LATENCY_CONSENT_MS: [0, 600_000],
} as const;

/** Whether `name` is a rendezvous file's: as the agent names them, so a file it returns lands only in the rendezvous folder. */
export function rendezvousFile(name: string): boolean {
	return /^[A-Za-z0-9_-]{1,64}\.json$/.test(name) && !/^(con|prn|aux|nul|com[1-9]|lpt[1-9])\./i.test(name);
}

/**
 * The rendezvous files a side on the agent takes, by role: what the other
 * role and the runner publish (`accepted` in the test kit's
 * `src/peer/scenarios/launch.rs`, which also checks what each holds).
 */
export const agentSideTakes = {
	host: ['viewer', 'viewer-streaming', 'read', 'done'],
	viewer: ['host', 'host-ready', 'read', 'read-done', 'done'],
} as const;

/** The key an invitation token (`vnl1.` and hex) was made by, as the agent reads it. */
export function invitationKey(token: unknown): string | undefined {
	const key = typeof token === 'string' && token.startsWith('vnl1.') ? token.slice(5, 69).toLowerCase() : '';
	return /^[0-9a-f]{64}$/.test(key) ? key : undefined;
}

/** The environment a latency side on the agent's computer runs with: what the agent set, and the suite's whitelisted numbers. */
export function agentLatencyEnvironment(role: 'host' | 'viewer', environment: NodeJS.ProcessEnv): Record<string, string> {
	// Only the agent starts a side here, and its other half is always on another computer.
	const result: Record<string, string> = { VNILINK_LATENCY_ROLE: role, VNILINK_LATENCY_ACROSS: '1' };
	for (const name of ['VNILINK_LATENCY_DIR', 'VNILINK_LATENCY_OUTPUT', 'VNILINK_LATENCY_SCREEN_GRANT', 'VNILINK_TEST_PROFILE']) {
		const value = environment[name];
		if (!value) {
			throw new Error(`The agent sets ${name} for a latency side`);
		}
		result[name] = value;
	}
	for (const [name, [least, most]] of Object.entries(agentLatencyVariables)) {
		const value = environment[name];
		if (value !== undefined && /^\d{1,9}$/.test(value) && Number(value) >= least && Number(value) <= most) {
			result[name] = value;
		}
	}
	return result;
}

/**
 * One side of a two-computer latency run on the test agent's computer
 * (`testkit:launch latency-host|latency-viewer`), started as a run on one
 * computer starts it, until `stopped` settles; the side then quits by
 * itself on the `done` the agent publishes, or is ended.
 */
export async function launchLatencySide(root: string, signal: AbortSignal, role: 'host' | 'viewer', stopped: Promise<void>): Promise<void> {
	await assertNativeFresh(root);
	await assertBundleFresh(root, 'latency.cjs');
	const environment = agentLatencyEnvironment(role, process.env);
	const processes = new ProcessRunner(signal);
	try {
		const platform = await latencyPlatform(root, undefined, processes, signal);
		const side = await platform.launch(role, environment, join(environment.VNILINK_LATENCY_OUTPUT, `${role}.log`));
		await Promise.race([side.result, stopped]);
		await settledWithin(side.result, 20000);
	} finally {
		await processes.stop();
	}
}
