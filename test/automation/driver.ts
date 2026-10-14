/** Owns readiness and renderer automation for one real Electron window. */
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { BrowserWindow } from 'electron';
import { Event } from '../../src/base/common/event.js';
import { Disposable } from '../../src/base/common/lifecycle.js';

export class WindowDriver extends Disposable {
	private bundleLoaded = false;

	constructor(public readonly window: BrowserWindow, private readonly output: string) {
		super();
		this._register(Event.fromNodeEventEmitter<unknown>(window.webContents, 'did-start-navigation')(() => {
			this.bundleLoaded = false;
		}));
	}

	public async ready(): Promise<void> {
		await this.window.webContents.executeJavaScript(`new Promise((resolve, reject) => {
            if (document.documentElement.dataset.workbenchReady === 'true') { resolve(true); return; }
            const observer = new MutationObserver(() => {
                if (document.documentElement.dataset.workbenchReady === 'true') { observer.disconnect(); clearTimeout(timer); resolve(true); }
            });
            const timer = setTimeout(() => { observer.disconnect(); reject(new Error('Workbench did not become ready')); }, 10000);
            observer.observe(document.documentElement, { attributes: true, childList: true, subtree: true });
        })`);
	}

	public async browserScenario(name: string): Promise<void> {
		await this.ready();
		if (!this.bundleLoaded) {
			await this.window.webContents.executeJavaScript(readFileSync(join(__dirname, 'browser-tests.js'), 'utf8'));
			this.bundleLoaded = true;
		}
		const result: unknown = await this.window.webContents.executeJavaScript(`browserTests.browserScenarios.run(${JSON.stringify(name)})`);
		if (result !== true) {
			throw new Error(`Browser scenario failed: ${name}`);
		}
	}

	public async screenshot(name: string): Promise<void> {
		const image = await this.window.webContents.capturePage();
		writeFileSync(join(this.output, `${name}.png`), image.toPNG());
	}
}
