/** Launches an unmodified assembled application outside the source tree, with isolated state. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { ProcessRunner } from '../../build/lib/processes.mjs';
import { assemblyEnvironment, packagedLayout } from '../../build/lib/packageConfiguration.mjs';
import { DevToolsConnection } from './devtools.mjs';
import { PackageVerifier } from '../../build/lib/packageVerifier.mjs';

export class PackagedApplication {
	constructor(root) {
		this.root = root;
		this.processes = new ProcessRunner();
		this.devtools = new DevToolsConnection();
	}

	async until(check, result) {
		const deadline = Date.now() + 90000;
		while (Date.now() < deadline) {
			if (result.exited) {
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

	async run(directory = join(this.root, 'dist', `${process.platform}-${process.arch}`)) {
		const product = JSON.parse(await readFile(join(this.root, 'product.json'), 'utf8'));
		const manifest = await new PackageVerifier(directory, product).verifyFiles();
		assert.equal(manifest.appId, product.appId);
		assert.equal(manifest.platform, process.platform);
		assert.equal(manifest.architecture, process.arch);
		const source = packagedLayout(directory, process.platform, product);
		assert.equal(createHash('sha256').update(await readFile(source.asar)).digest('hex'), manifest.asarSha256);
		assert.equal(createHash('sha256').update(await readFile(source.native)).digest('hex'), manifest.nativeSha256);
		const work = await mkdtemp(join(tmpdir(), 'vnilink-package-smoke-'));
		const artifacts = join(this.root, '.build/package-tests', `${Date.now()}-${process.pid}`);
		await mkdir(artifacts, { recursive: true });
		try {
			const copy = join(work, 'application');
			await cp(directory, copy, { recursive: true, verbatimSymlinks: true });
			const profile = join(work, 'profile');
			await mkdir(profile, { mode: 0o700 });
			const layout = packagedLayout(copy, process.platform, product);
			const launched = this.processes.start(layout.executable, ['--smoke-test', '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1'], { cwd: work, env: { ...assemblyEnvironment(process.env), VNILINK_TEST_PROFILE: profile }, stdio: ['ignore', 'pipe', 'pipe'] });
			let log = '';
			const collect = bytes => {
				log = (log + bytes.toString()).slice(-65536);
			};
			launched.child.stdout.on('data', collect);
			launched.child.stderr.on('data', collect);
			const result = { exited: false };
			void launched.result.finally(() => {
				result.exited = true;
			}).catch(() => undefined);
			try {
				const port = await this.until(async () => {
					try {
						return Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
					} catch (error) {
						if (error.code !== 'ENOENT') {
							throw error;
						}
						return undefined;
					}
				}, result);
				assert(Number.isInteger(port) && port > 0 && port <= 65535);
				const target = await this.until(async () => {
					const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(5000) })).json();
					return targets.find(value => value.type === 'page' && value.url === 'vnilink://app/index.html');
				}, result);
				await this.devtools.connect(target.webSocketDebuggerUrl);
				await this.until(() => this.devtools.evaluate(`document.documentElement?.dataset.workbenchReady === 'true'`), result);
				const state = await this.devtools.evaluate(`(async () => ({ info: await window.vscode.context.resolveConfiguration(), snapshot: await window.driver.engine.execute({type:'snapshot'}), heading: document.querySelector('h1')?.textContent, noticeHidden: document.querySelector('.workbench-notice')?.hidden, nodeExposed: typeof require !== 'undefined' || typeof process !== 'undefined' }))()`);
				assert.equal(state.info.engineAvailable, true);
				assert.equal(state.info.version, manifest.version);
				assert.equal(state.info.platform, process.platform);
				assert.equal(state.snapshot.type, 'snapshot');
				assert.equal(state.snapshot.snapshot.devices.length, 0);
				assert.equal(state.heading, 'Devices');
				assert.equal(state.noticeHidden, true);
				assert.equal(state.nodeExposed, false);
				const screenshot = await this.devtools.request('Page.captureScreenshot', { format: 'png' });
				await writeFile(join(artifacts, 'devices.png'), Buffer.from(screenshot.data, 'base64'));
				await writeFile(join(artifacts, 'result.json'), JSON.stringify({ platform: process.platform, architecture: process.arch, appId: product.appId, version: manifest.version, engineAvailable: state.info.engineAvailable, nodeExposed: state.nodeExposed, launchedOutsideCheckout: true }, null, 2) + '\n');
			} finally {
				await writeFile(join(artifacts, 'application.log'), log);
				try {
					await this.devtools.request('Browser.close');
				} catch {
					// A browser close can retire the connection before replying.
				}
				this.devtools.dispose();
				await this.processes.stop();
			}
			console.log(`Packaged application passed isolated launch checks; evidence: ${artifacts}`);
		} finally {
			this.devtools.dispose();
			await this.processes.stop();
			await rm(work, { recursive: true, force: true });
		}
	}

	stop() {
		this.devtools.dispose();
		return this.processes.stop();
	}
}
