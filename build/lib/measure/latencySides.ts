// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** How a latency run starts its sides and reader on each OS, and how the sides meet. */
import { access, open, readdir, readFile, rename, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { electronExecutable } from '../electron.ts';
import { defaultNativeOptions, nativeArtifact, nativeCargo } from '../native.ts';
import { isLinux, isMacintosh, isWindows } from '../platform.ts';
import type { ProcessRunner } from '../processes.ts';
import { type LaunchedProcess } from '../processes.ts';
import type { AgentSession } from '../remote.ts';
import { agentSideTakes, rendezvousFile } from './latencyAgent.ts';

/** Waits until `work` settles or `ms` pass, whichever is first, leaving no timer behind to keep the process alive. */
export async function settledWithin(work: Promise<unknown>, ms: number): Promise<void> {
	let timer: NodeJS.Timeout | undefined;
	try {
		await Promise.race([work.catch(() => undefined), new Promise(resolve => {
			timer = setTimeout(resolve, ms);
		})]);
	} finally {
		clearTimeout(timer);
	}
}

/** Publishes `value` under `name` for the sides, whole or not at all. */
export async function publish(directory: string, name: string, value: unknown): Promise<void> {
	const path = join(directory, `${name}.json`);
	await writeFile(`${path}.partial`, JSON.stringify(value));
	await rename(`${path}.partial`, path);
}

/** Waits for `name` in the rendezvous directory, or fails when either side has exited. */
export async function arrival(directory: string, name: string, timeoutMs: number, exited: () => boolean, signal: AbortSignal): Promise<void> {
	const deadline = Date.now() + timeoutMs;
	while (!await access(join(directory, `${name}.json`)).then(() => true, () => false)) {
		signal.throwIfAborted();
		if (exited()) {
			throw new Error(`A side exited before ${name}`);
		}
		if (Date.now() > deadline) {
			throw new Error(`${name} did not arrive within ${String(timeoutMs / 1000)} s`);
		}
		await new Promise(resolve => setTimeout(resolve, 200));
	}
}

/**
 * The cargo command and environment that build the `glass_latency` example of
 * `crate` for `target`, the target and tools the addon build uses, so the
 * reader finds the same native libraries and lands where `reader` expects it.
 */
export function readerBuild(
	crate: string,
	target: string | undefined,
	environment: NodeJS.ProcessEnv = process.env,
	exists?: (path: string) => boolean,
): { readonly args: string[]; readonly env: NodeJS.ProcessEnv } {
	return nativeCargo('build', ['--release', '-p', crate, '--example', 'glass_latency'], { profile: 'dev', target }, environment, exists);
}

/** How one platform runs the two sides and the reader. */
interface LatencyPlatform {
	/** The crate whose `glass_latency` example reads this platform's display. */
	readonly readerCrate: string;
	/** The cargo target the reader is built for, when it is not the host's default. */
	readonly readerTarget?: string;
	readonly reader: string;
	/** How long someone at this computer may take to answer a consent dialog, for the host and for the reader. */
	readonly consentMs: number;
	/** Starts one side; its result settles when the side has quit. */
	launch(role: 'host' | 'viewer', environment: Readonly<Record<string, string>>, log: string): Promise<LaunchedProcess>;
}

/** `app`: on macOS, the `.app` that runs the sides; this checkout's development VniLink when undefined. */
export async function latencyPlatform(root: string, app: string | undefined, processes: ProcessRunner, signal: AbortSignal): Promise<LatencyPlatform> {
	const entry = join(root, 'out/latency.cjs');
	if (isMacintosh) {
		// .../VniLink.app/Contents/MacOS/Electron
		const bundle = app ?? dirname(dirname(dirname(await electronExecutable(root, signal))));
		return {
			readerCrate: 'vnilink-macos',
			reader: nativeArtifact(root, { profile: 'dev' }, 'release', 'examples/glass_latency'),
			// macOS asks before the run; a refusal stops it.
			consentMs: 0,
			// Launched through LaunchServices, each side is its own responsible
			// application, so macOS asks Screen Recording of the development
			// VniLink and not of whatever started this task. `open -W` waits for it.
			launch: (_role, environment, log) => Promise.resolve(processes.start('open', [
				'-n', '-W', '-g', '-a', bundle,
				...Object.entries(environment).flatMap(([name, value]) => ['--env', `${name}=${value}`]),
				'--stdout', log, '--stderr', log,
				'--args', entry,
			])),
		};
	}
	if (app !== undefined) {
		throw new Error('--app names a macOS application');
	}
	const electron = await electronExecutable(root, signal);
	const direct = (args: (role: 'host' | 'viewer', environment: Readonly<Record<string, string>>) => readonly string[]): LatencyPlatform['launch'] => async (role, environment, log) => {
		// The side writes to the log itself; the descriptor is closed here once it has its own.
		const output = await open(log, 'a');
		try {
			return processes.start(electron, [...args(role, environment), entry], { env: { ...process.env, ...environment }, stdio: ['ignore', output.fd, output.fd] });
		} finally {
			await output.close();
		}
	};
	if (isWindows) {
		// Windows asks no permission to capture; Desktop Duplication needs the
		// signed-in desktop, which a service or a remote shell is not in.
		const { target } = defaultNativeOptions();
		return {
			readerCrate: 'vnilink-windows',
			readerTarget: target,
			reader: nativeArtifact(root, { profile: 'dev', target }, 'release', 'examples/glass_latency.exe'),
			consentMs: 0,
			launch: direct(() => []),
		};
	}
	if (isLinux) {
		// The portal asks someone at this computer twice, for the host's
		// screen and for the reader's, and needs the graphical session's
		// bus. The sides run under XWayland: a Wayland client cannot place
		// the stimulus at the top of the display or the viewer below it.
		// Across two computers the viewer reads no screen, so it needs no
		// placed window and runs as a Wayland client: Chromium decodes in
		// hardware through VA-API there, which is what a viewer measured
		// across two computers should show. Under XWayland it decodes in
		// software.
		return {
			readerCrate: 'vnilink-linux',
			reader: nativeArtifact(root, { profile: 'dev' }, 'release', 'examples/glass_latency'),
			consentMs: 5 * 60000,
			launch: direct((role, environment) => [role === 'viewer' && environment.VNILINK_LATENCY_ACROSS === '1' ? '--ozone-platform=wayland' : '--ozone-platform=x11']),
		};
	}
	throw new Error('measure:latency reads a macOS, Windows or Linux display');
}

/**
 * Carries each side's rendezvous files to the other through the agent
 * until `stopped`: both sides write a file once, whole, by renaming, so a
 * file is carried once. `running` hears whether the agent's side still runs.
 */
export async function relayRendezvous(
	session: AgentSession, here: string, there: 'host' | 'viewer', stopped: () => boolean, running: (running: boolean) => void, signal: AbortSignal,
): Promise<void> {
	const carried = new Set<string>();
	const taken = agentSideTakes[there].map(name => `${name}.json`);
	while (!stopped()) {
		signal.throwIfAborted();
		for (const name of (await readdir(here)).filter(name => taken.includes(name) && !carried.has(name))) {
			await session.put(name, await readFile(join(here, name)));
			carried.add(name);
		}
		const polled = await session.poll();
		const strange = polled.files.find(file => !rendezvousFile(file.name));
		if (strange) {
			throw new Error(`The agent returned a file that is no rendezvous file: ${JSON.stringify(strange.name.slice(0, 80))}`);
		}
		for (const file of polled.files.filter(file => !carried.has(file.name))) {
			await writeFile(join(here, `${file.name}.partial`), file.data);
			await rename(join(here, `${file.name}.partial`), join(here, file.name));
			carried.add(file.name);
		}
		running(polled.running);
		await new Promise(resolve => setTimeout(resolve, 250));
	}
}
