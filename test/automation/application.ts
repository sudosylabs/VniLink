/** Isolated application profile, diagnostics and total smoke-run cleanup. */
import { app, session, type BrowserWindow } from 'electron';
import { appendFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { VniLinkApplication } from '../../src/app/electron-main/application.js';
import { WindowDriver } from './driver.js';

export class TestApplication {
	public readonly profile = process.env.VNILINK_TEST_PROFILE ?? mkdtempSync(join(tmpdir(), 'vnilink-test-'));
	public readonly output = join(__dirname, '../.build/test-runs', `${String(Date.now())}-${String(process.pid)}`);
	private timer: ReturnType<typeof setTimeout> | undefined;
	private readonly container = process.platform === 'linux' && process.env.VNILINK_TEST_CONTAINER === '1';

	public get args(): string[] {
		// Electron retains launch switches in argv; they are not scenario names.
		const args = process.argv.slice(2);
		return this.container ? args.filter(arg => arg !== '--no-sandbox') : args;
	}

	constructor() {
		app.setPath('userData', this.profile);
		mkdirSync(this.output, { recursive: true });
		writeFileSync(join(this.output, 'environment.json'), JSON.stringify({ platform: process.platform, architecture: process.arch, versions: process.versions, chromiumSandbox: !this.container }, null, 2));
	}

	public async run(verify: (window: BrowserWindow, driver: WindowDriver) => Promise<void>): Promise<void> {
		this.timer = setTimeout(() => {
			this.cleanup();
			app.exit(1);
		}, 120000);
		app.once('quit', () => {
			this.cleanup();
		});
		app.on('web-contents-created', (_event, contents) => {
			contents.on('did-fail-load', (_event, code, description, url) => {
				appendFileSync(join(this.output, 'navigation.log'), `${String(code)} ${description} ${url}\n`);
			});
			contents.on('console-message', details => {
				if (details.level === 'error') {
					appendFileSync(join(this.output, 'console.log'), `${details.message}\n`);
				}
			});
		});
		app.once('ready', () => {
			session.defaultSession.webRequest.onErrorOccurred(details => {
				appendFileSync(join(this.output, 'network.log'), `${details.error} ${details.url}\n`);
			});
		});
		await new VniLinkApplication({
			showWindow: false, temporaryEngine: true, onReady: async window => {
				const driver = new WindowDriver(window, this.output);
				try {
					await driver.ready();
					await verify(window, driver);
					console.log(`PASS: test scenarios; artifacts: ${this.output}`);
				} catch (error) {
					appendFileSync(join(this.output, 'failure.log'), error instanceof Error ? error.stack ?? error.message : String(error));
					await driver.screenshot('failure').catch(() => undefined);
					throw error;
				} finally {
					driver.dispose();
				}
				app.quit();
			}
		}).start();
	}

	private cleanup(): void {
		clearTimeout(this.timer);
		if (!process.env.VNILINK_TEST_PROFILE) {
			rmSync(this.profile, { recursive: true, force: true });
		}
	}
}
