// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The renderer of `test:unit:electron`, after VS Code's
 * `test/unit/electron/renderer.js`: mocha's browser build with the TDD
 * interface loads each test bundle (and its styles) as a script, runs them and
 * sends every result to the main process (`main.ts`).
 */
import { ipcRenderer } from 'electron';
import { join } from 'node:path';

declare const mocha: BrowserMocha;

declare const Mocha: { readonly reporters: { readonly Base: new (runner: MochaRunner) => object } };

interface BrowserMocha {
	setup(options: { ui: 'tdd'; timeout: number; grep?: RegExp; reporter: new (runner: MochaRunner) => object }): void;
	run(): MochaRunner;
}

interface MochaRunner {
	on(event: 'test' | 'pass' | 'pending', listener: (test: MochaTest) => void): void;
	on(event: 'fail', listener: (test: MochaTest, error: Error) => void): void;
	on(event: 'end', listener: () => void): void;
	readonly stats?: { passes: number; failures: number; pending: number };
}

interface MochaTest {
	fullTitle(): string;
	readonly duration?: number;
}

const output = process.env.VNILINK_UNIT_OUTPUT ?? '';

const files = JSON.parse(process.env.VNILINK_UNIT_FILES ?? '[]') as string[];

const grep = process.env.VNILINK_UNIT_GREP;

/** What was running when an error no test's promise carried arrived. */
let running = 'loading the unit tests';

/** Errors no test's promise carried, each already reported as a failure. */
let unexpectedErrors = 0;

function send(channel: 'vnilink:unit:result' | 'vnilink:unit:end', message: object): void {
	ipcRenderer.send(channel, message);
}

/**
 * Reports an error no test's promise carried (a window error, as Chromium
 * reports a ResizeObserver loop, a rejection nobody handled, or a bundle
 * that throws while it loads) as a failure of what was running. As in VS
 * Code's runner it fails the run; unlike mocha's own handler, it does not
 * stop the tests after it.
 */
function unexpected(error: unknown, fallback: string): void {
	unexpectedErrors++;
	const stack = error instanceof Error ? error.stack ?? error.message : fallback;
	send('vnilink:unit:result', { title: `${running}: an error outside its promise`, state: 'failed', error: stack });
}

window.addEventListener('error', event => {
	// A script or stylesheet that fails to load reaches the window here too, on its way to the element.
	if (!(event instanceof ErrorEvent)) {
		return;
	}
	event.preventDefault();
	event.stopImmediatePropagation();
	// Chromium's report that observers resized within one frame; it delivers the rest on the next frame, losing nothing.
	if (event.message.startsWith('ResizeObserver loop')) {
		return;
	}
	unexpected(event.error, event.message);
}, { capture: true });

window.addEventListener('unhandledrejection', event => {
	event.preventDefault();
	const reason: unknown = event.reason;
	// A cancelled operation is how disposal ends work; upstream ignores it too.
	if (reason instanceof Error && reason.name === 'Canceled') {
		return;
	}
	unexpected(reason, String(reason));
});

function load(element: HTMLScriptElement | HTMLLinkElement): Promise<boolean> {
	return new Promise(resolve => {
		element.addEventListener('load', () => {
			resolve(true);
		}, { once: true });
		element.addEventListener('error', () => {
			resolve(false);
		}, { once: true });
		document.head.append(element);
	});
}

async function run(): Promise<void> {
	// Results go to the main process below; mocha's base reporter only counts them.
	mocha.setup({ ui: 'tdd', timeout: Number(process.env.VNILINK_UNIT_TIMEOUT), grep: grep ? new RegExp(grep) : undefined, reporter: Mocha.reporters.Base });
	for (const file of files) {
		const style = document.createElement('link');
		style.rel = 'stylesheet';
		style.href = join(output, file.replace(/\.js$/, '.css'));
		// A test whose subject imports no styles has no stylesheet; that is not a failure.
		await load(style);
		const script = document.createElement('script');
		script.src = join(output, file);
		if (!await load(script)) {
			throw new Error(`The test bundle ${file} did not load`);
		}
	}
	const runner = mocha.run();
	runner.on('test', test => {
		running = test.fullTitle();
	});
	runner.on('pass', test => {
		send('vnilink:unit:result', { title: test.fullTitle(), state: 'passed', duration: test.duration });
	});
	runner.on('pending', test => {
		send('vnilink:unit:result', { title: test.fullTitle(), state: 'pending' });
	});
	runner.on('fail', (test, error) => {
		send('vnilink:unit:result', { title: test.fullTitle(), state: 'failed', error: error.stack ?? String(error) });
	});
	runner.on('end', () => {
		const stats = runner.stats ?? { passes: 0, failures: 1, pending: 0 };
		send('vnilink:unit:end', { ...stats, failures: stats.failures + unexpectedErrors });
	});
}

run().catch((error: unknown) => {
	send('vnilink:unit:result', { title: 'loading the unit tests', state: 'failed', error: error instanceof Error ? error.stack : String(error) });
	send('vnilink:unit:end', { passes: 0, failures: 1, pending: 0 });
});
