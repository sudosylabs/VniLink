// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { BrowserWindow } from 'electron';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { VSBuffer } from '../../../../base/common/buffer.js';
import { Event } from '../../../../base/common/event.js';
import { DisposableStore } from '../../../../base/common/lifecycle.js';
import { URI } from '../../../../base/common/uri.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { IWindowsMainService } from '../../../windows/electron-main/windowsMainService.js';
import type { LogTail } from '../../common/native.js';
import { NativeHostChannel } from '../../electron-main/nativeHostChannel.js';
import { NativeHostMainService, type INativeShell, type TextClipboard } from '../../electron-main/nativeHostMainService.js';
import { upcastPartial } from '../../../../base/test/common/mock.js';

/** A clipboard that holds text in memory. */
class TestClipboard implements TextClipboard {
	public text = '';

	public readText(): Promise<string> {
		return Promise.resolve(this.text);
	}

	public writeText(text: string): Promise<void> {
		this.text = text;
		return Promise.resolve();
	}
}

/** The shell and clipboards, recording what the service asked of them; no selection clipboard, as off Linux. */
class TestNativeShell implements INativeShell {
	public readonly clipboard = new TestClipboard();
	public selectionClipboard: TestClipboard | undefined;
	public readonly pictures: Uint8Array[] = [];
	public readonly opened: string[] = [];
	public readonly shown: string[] = [];

	public writeClipboardPng(png: Uint8Array): Promise<void> {
		this.pictures.push(png);
		return Promise.resolve();
	}

	public openExternal(url: string): Promise<void> {
		this.opened.push(url);
		return Promise.resolve();
	}

	public showItemInFolder(path: string): void {
		this.shown.push(path);
	}
}

const png = VSBuffer.wrap(Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]));

/** Clipboard writes from window 7 that main refuses. */
const refusedWrites: readonly (readonly [string, unknown[]])[] = [
	['for another window', [8, 'other window']],
	['of a number', [7, 42]],
	['of more than 16 MiB', [7, 'x'.repeat(16 * 1024 * 1024 + 1)]],
	['to the find clipboard', [7, 'text', 'find']],
	['with an extra argument', [7, 'text', 'clipboard', 'extra']],
];

suite('NativeHostChannel', () => {
	const disposables = new DisposableStore();
	let nativeShell: TestNativeShell;
	let relaunches: number;

	/** The channel window 7 talks to, on a service whose logs folder is `logsHome`. */
	function createChannel(logsHome = '/logs/run'): NativeHostChannel {
		const instantiationService = disposables.add(new TestInstantiationService());
		const window = upcastPartial<BrowserWindow>({ id: 7 });
		instantiationService.stub(IWindowsMainService, { onDidOpenWindow: Event.None, getWindowById: id => id === 7 ? window : undefined });
		const nativeHost = disposables.add(new NativeHostMainService(URI.file(logsHome), instantiationService.get(IWindowsMainService), nativeShell));
		return new NativeHostChannel(nativeHost, () => {
			relaunches++;
			return Promise.resolve();
		});
	}
	setup(() => {
		nativeShell = new TestNativeShell();
		relaunches = 0;
	});
	teardown(() => {
		disposables.clear();
	});
	test('a request naming another window is refused', async () => {
		await assert.rejects(createChannel().call('window:7', 'getWindowState', [8]), /Invalid/);
	});
	test('an unknown request is refused', async () => {
		await assert.rejects(createChannel().call('window:7', 'invalid', [7]), /Invalid/);
	});
	test('a request from outside an application window is refused', async () => {
		await assert.rejects(createChannel().call('renderer', 'relaunch', [7]));
		assert.strictEqual(relaunches, 0);
	});
	test('a window can relaunch the application', async () => {
		await createChannel().call('window:7', 'relaunch', [7]);
		assert.strictEqual(relaunches, 1);
	});
	test('a link that is not allowed is not opened', async () => {
		await assert.rejects(createChannel().call('window:7', 'openExternal', [7, 'https://arbitrary.example/']), /not allowed/);
		assert.deepStrictEqual(nativeShell.opened, []);
	});
	test('an allowed link opens in the system browser', async () => {
		await createChannel().call('window:7', 'openExternal', [7, 'https://docs.iroh.computer/relays/rate-limiting']);
		assert.deepStrictEqual(nativeShell.opened, ['https://docs.iroh.computer/relays/rate-limiting']);
	});
	test('the logs folder opens at the main log', async () => {
		await createChannel().call('window:7', 'openLogsFolder', [7]);
		assert.deepStrictEqual(nativeShell.shown, [join(URI.file('/logs/run').fsPath, 'main.log')]);
	});
	test('the logs folder takes no path from the window', async () => {
		await assert.rejects(createChannel().call('window:7', 'openLogsFolder', [7, '/elsewhere']), /Invalid/);
		assert.deepStrictEqual(nativeShell.shown, []);
	});
	test('a window writes plain text to the clipboard and reads it back', async () => {
		const channel = createChannel();
		await channel.call('window:7', 'writeClipboardText', [7, 'copied ✓']);
		assert.strictEqual(await channel.call('window:7', 'readClipboardText', [7]), 'copied ✓');
	});
	test('the selection reads the system clipboard where there is no selection clipboard', async () => {
		const channel = createChannel();
		await channel.call('window:7', 'writeClipboardText', [7, 'copied ✓']);
		assert.strictEqual(await channel.call('window:7', 'readClipboardText', [7, 'selection']), 'copied ✓');
	});
	test('the selection uses the selection clipboard where there is one', async () => {
		nativeShell.selectionClipboard = new TestClipboard();
		const channel = createChannel();
		await channel.call('window:7', 'writeClipboardText', [7, 'selected', 'selection']);
		assert.strictEqual(nativeShell.selectionClipboard.text, 'selected');
		assert.strictEqual(nativeShell.clipboard.text, '');
	});
	for (const [name, args] of refusedWrites) {
		test(`a clipboard write ${name} is refused and changes nothing`, async () => {
			const channel = createChannel();
			await channel.call('window:7', 'writeClipboardText', [7, 'copied ✓']);
			await assert.rejects(channel.call('window:7', 'writeClipboardText', args), /Invalid/);
			assert.strictEqual(await channel.call('window:7', 'readClipboardText', [7]), 'copied ✓');
		});
	}
	test('a clipboard read of anything but text is refused', async () => {
		await assert.rejects(createChannel().call('window:7', 'readClipboardText', [7, 'files']), /Invalid/);
	});
	test('a clipboard read for another window is refused', async () => {
		await assert.rejects(createChannel().call('window:7', 'readClipboardText', [8]), /Invalid/);
	});
	test('a window copies a PNG picture to the clipboard', async () => {
		await createChannel().call('window:7', 'writeClipboardPicture', [7, png]);
		assert.deepStrictEqual(nativeShell.pictures, [Uint8Array.from(png.buffer)]);
	});
	test('a picture that is not a PNG is not copied', async () => {
		await assert.rejects(createChannel().call('window:7', 'writeClipboardPicture', [7, VSBuffer.fromString('not a picture')]), /Invalid/);
		assert.deepStrictEqual(nativeShell.pictures, []);
	});
	test('a picture for another window is not copied', async () => {
		await assert.rejects(createChannel().call('window:7', 'writeClipboardPicture', [8, png]), /Invalid/);
		assert.deepStrictEqual(nativeShell.pictures, []);
	});
	test('pictures are not saved through the native host', async () => {
		await assert.rejects(createChannel().call('window:7', 'savePicture', [7, 'picture.png', png]), /Invalid/);
	});
	suite('diagnostics export', () => {
		let logs: string;
		setup(async () => {
			logs = await mkdtemp(join(tmpdir(), 'vnilink-logs-'));
			await mkdir(join(logs, 'window1'), { recursive: true });
			await writeFile(join(logs, 'main.log'), `started\nopened ${homedir()}/Library/x\n`);
			await writeFile(join(logs, 'window1', 'renderer.log'), 'x'.repeat(300 * 1024) + 'last');
		});
		teardown(async () => {
			await rm(logs, { recursive: true, force: true });
		});
		function readTails(): Promise<LogTail[]> {
			return createChannel(logs).call<LogTail[]>('window:7', 'readLogTails', [7]);
		}
		test('it reads the main log and each window\'s log of this run', async () => {
			assert.deepStrictEqual((await readTails()).map(tail => tail.name).sort(), ['main.log', join('window1', 'renderer.log')]);
		});
		test('it writes the home folder as ~', async () => {
			assert.strictEqual((await readTails()).find(tail => tail.name === 'main.log')?.text, 'started\nopened ~/Library/x\n');
		});
		test('it keeps the last 256 KiB of a longer log', async () => {
			const renderer = (await readTails()).find(tail => tail.name !== 'main.log')?.text ?? '';
			assert.strictEqual(renderer.length, 256 * 1024);
			assert.ok(renderer.endsWith('last'));
		});
		test('it takes no file name from the window', async () => {
			await assert.rejects(createChannel(logs).call('window:7', 'readLogTails', [7, 'main.log']), /Invalid/);
		});
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});
