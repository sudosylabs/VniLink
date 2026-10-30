// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Launches an unmodified assembled application outside the source tree, with isolated state. */
import assert from 'node:assert/strict';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { release, tmpdir } from 'node:os';
import { join } from 'node:path';
import { DevToolsConnection } from './devtools.ts';
import { assemblyEnvironment, packagedLayout } from './package.ts';
import { ProcessRunner } from './processes.ts';
import { readProduct } from './product.ts';
import { fileSha256, verifyPackageFiles } from './verify.ts';

interface ApplicationState {
	readonly info: { readonly engineAvailable: boolean; readonly version: string; readonly platform: string };
	readonly snapshot: { readonly type: string; readonly snapshot: { readonly devices: readonly unknown[] } };
	readonly heading: string | undefined;
	readonly noticeHidden: boolean | undefined;
	readonly nodeExposed: boolean;
	readonly material: string | undefined;
}

/** Polls `check` until it returns a value, failing if the application exits first or 90 s pass. */
async function until<T>(check: () => Promise<T | undefined>, exited: () => boolean): Promise<T> {
	const deadline = Date.now() + 90000;
	while (Date.now() < deadline) {
		if (exited()) {
			throw new Error('Packaged application exited before readiness');
		}
		const value = await check();
		if (value) {
			return value;
		}
		await new Promise(resolve => setTimeout(resolve, 100));
	}
	throw new Error('Packaged application readiness timed out');
}

/** Copies the application in `directory` to a temporary location, launches it with an isolated profile and checks the workbench. */
export async function runPackagedApplication(root: string, directory: string, signal: AbortSignal): Promise<void> {
	const product = await readProduct(root);
	const manifest = await verifyPackageFiles(directory, product);
	const source = packagedLayout(directory, process.platform, product);
	assert.equal(await fileSha256(source.asar), manifest.asarSha256);
	assert.equal(await fileSha256(source.native), manifest.nativeSha256);
	const processes = new ProcessRunner(signal);
	const devtools = new DevToolsConnection();
	const work = await mkdtemp(join(tmpdir(), 'vnilink-package-smoke-'));
	const artifacts = join(root, '.build/package-tests', `${String(Date.now())}-${String(process.pid)}`);
	await mkdir(artifacts, { recursive: true });
	let log = '';
	try {
		const copy = join(work, 'application');
		await cp(directory, copy, { recursive: true, verbatimSymlinks: true });
		const profile = join(work, 'profile');
		await mkdir(profile, { mode: 0o700 });
		const layout = packagedLayout(copy, process.platform, product);
		const launched = processes.start(layout.executable, ['--smoke-test', '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1'], { cwd: work, env: { ...assemblyEnvironment(process.env), VNILINK_TEST_PROFILE: profile }, stdio: ['ignore', 'pipe', 'pipe'] });
		const collect = (bytes: Buffer): void => {
			log = (log + bytes.toString()).slice(-65536);
		};
		launched.child.stdout?.on('data', collect);
		launched.child.stderr?.on('data', collect);
		let exited = false;
		launched.result.finally(() => {
			exited = true;
		}).catch(() => undefined);
		const port = await until(async () => {
			try {
				return Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
					throw error;
				}
				return undefined;
			}
		}, () => exited);
		assert(Number.isInteger(port) && port > 0 && port <= 65535);
		const target = await until(async () => {
			const targets = await (await fetch(`http://127.0.0.1:${String(port)}/json/list`, { signal: AbortSignal.timeout(5000) })).json() as { type: string; url: string; webSocketDebuggerUrl: string }[];
			return targets.find(value => value.type === 'page' && value.url === 'vnilink://app/index.html');
		}, () => exited);
		await devtools.connect(target.webSocketDebuggerUrl);
		await until(() => devtools.evaluate<boolean>(`document.documentElement?.dataset.workbenchReady === 'true'`), () => exited);
		const state = await devtools.evaluate<ApplicationState>(`(async () => ({ info: await window.vscode.context.resolveConfiguration(), snapshot: await window.driver.engine.execute({type:'snapshot'}), heading: document.querySelector('h1')?.textContent, noticeHidden: document.querySelector('.workbench-notice')?.hidden, nodeExposed: typeof require !== 'undefined' || typeof process !== 'undefined', material: document.documentElement.dataset.windowMaterial }))()`);
		assert.equal(state.info.engineAvailable, true);
		assert.equal(state.info.version, manifest.version);
		assert.equal(state.info.platform, process.platform);
		assert.equal(state.snapshot.type, 'snapshot');
		assert.equal(state.snapshot.snapshot.devices.length, 0);
		assert.equal(state.heading, 'Devices');
		assert.equal(state.noticeHidden, true);
		assert.equal(state.nodeExposed, false);
		// macOS 26 (Darwin 25) and later ship Liquid Glass; vibrancy there means elg did not load
		// from the package. Reduce Transparency may still turn the material off.
		if (process.platform === 'darwin' && Number(release().split('.')[0]) >= 25) {
			assert.ok(state.material === 'glass' || state.material === 'none', `The packaged window has ${String(state.material)} instead of Liquid Glass`);
		}
		const screenshot = await devtools.request<{ data: string }>('Page.captureScreenshot', { format: 'png' });
		await writeFile(join(artifacts, 'devices.png'), Buffer.from(screenshot.data, 'base64'));
		await writeFile(join(artifacts, 'result.json'), JSON.stringify({ platform: process.platform, architecture: process.arch, appId: product.appId, version: manifest.version, engineAvailable: state.info.engineAvailable, nodeExposed: state.nodeExposed, material: state.material, launchedOutsideCheckout: true }, null, 2) + '\n');
		console.log(`Packaged application passed isolated launch checks; evidence: ${artifacts}`);
	} finally {
		await writeFile(join(artifacts, 'application.log'), log);
		try {
			await devtools.request('Browser.close');
		} catch {
			// A browser close can retire the connection before replying.
		}
		devtools.dispose();
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}
