// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * `--self-check[=<directory>]`: what an assembled application must do,
 * checked from inside main, so a shipped build needs no automation driver
 * or debugging port to be verified. The application starts with a temporary
 * engine and profile, opens its window, checks it, prints one JSON line and
 * exits 0 when every check passed. With a directory, the result and a
 * screenshot of the window are written there too.
 */
import { app, type BrowserWindow } from 'electron';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { arch, platform } from '../../base/common/process.js';
import type { EngineCommand, EngineReply } from '../../platform/engine/common/engine.js';

export interface SelfCheckResult {
	readonly passed: boolean;
	readonly problems: readonly string[];
	readonly version: string;
	readonly platform: string;
	readonly architecture: string;
	readonly material: string | undefined;
}

interface WindowState {
	readonly configuration: { readonly engineAvailable?: unknown; readonly version?: unknown; readonly platform?: unknown } | undefined;
	readonly heading: string | undefined;
	readonly noticeHidden: boolean | undefined;
	readonly nodeExposed: boolean;
	readonly driverExposed: boolean;
	readonly material: string | undefined;
}

/** How long the workbench may take to become ready. */
const readyTimeout = 60_000;

/** The `--self-check` argument's directory: `''` without one, `undefined` when absent. */
export function selfCheckOutput(argv: readonly string[]): string | undefined {
	const argument = argv.find(value => value === '--self-check' || value.startsWith('--self-check='));
	return argument === undefined ? undefined : argument.slice('--self-check='.length);
}

async function whenWorkbenchReady(window: BrowserWindow): Promise<void> {
	const deadline = Date.now() + readyTimeout;
	while (Date.now() < deadline) {
		if (await window.webContents.executeJavaScript(`document.documentElement?.dataset.workbenchReady === 'true'`) === true) {
			return;
		}
		await new Promise(resolve => setTimeout(resolve, 100));
	}
	throw new Error('The workbench did not become ready');
}

interface SelfCheckEngine {
	execute(command: EngineCommand): Promise<EngineReply>;
}

export async function runSelfCheck(window: BrowserWindow, engine: SelfCheckEngine, output: string): Promise<SelfCheckResult> {
	const problems: string[] = [];
	const expect = (passed: boolean, problem: string): void => {
		if (!passed) {
			problems.push(problem);
		}
	};
	let material: string | undefined;
	try {
		await whenWorkbenchReady(window);
		const state = await window.webContents.executeJavaScript(`(async () => ({
			configuration: await window.vnilink?.context.resolveConfiguration(),
			heading: document.querySelector('h1')?.textContent,
			noticeHidden: document.querySelector('.workbench-notice')?.hidden,
			nodeExposed: typeof require !== 'undefined' || typeof process !== 'undefined',
			driverExposed: 'driver' in window,
			material: document.documentElement.dataset.windowMaterial,
		}))()`) as WindowState;
		material = state.material;
		expect(state.configuration?.engineAvailable === true, 'the window was told the engine is unavailable');
		expect(state.configuration?.version === app.getVersion(), 'the window reports another version');
		expect(state.configuration?.platform === platform, 'the window reports another platform');
		expect(state.heading === 'Devices', `the window shows ${String(state.heading)} instead of Devices`);
		expect(state.noticeHidden === true, 'the workbench notice is showing');
		expect(!state.nodeExposed, 'Node is reachable from the page');
		expect(!state.driverExposed, 'the automation driver is exposed');
		const snapshot = await engine.execute({ type: 'snapshot' });
		expect(snapshot.type === 'snapshot' && snapshot.snapshot.devices.length === 0, 'the engine did not answer a snapshot for a new profile');
		if (output) {
			await mkdir(output, { recursive: true });
			await writeFile(join(output, 'devices.png'), (await window.webContents.capturePage()).toPNG());
		}
	} catch (error) {
		problems.push(error instanceof Error ? error.message : String(error));
	}
	const result: SelfCheckResult = {
		passed: problems.length === 0, problems, version: app.getVersion(), platform, architecture: arch ?? 'unknown', material,
	};
	if (output) {
		await writeFile(join(output, 'result.json'), `${JSON.stringify(result, null, 2)}\n`);
	}
	return result;
}
