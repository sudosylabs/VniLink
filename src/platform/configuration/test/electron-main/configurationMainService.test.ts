// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdir, mkdtemp, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import type { EngineCommand, EngineReply } from '../../../engine/common/engine.js';
import { IEngineMainService } from '../../../engine/electron-main/engineMainService.js';
import { TestEngineMainService } from '../../../engine/test/electron-main/testEngineMainService.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../log/common/log.js';
import { ConfigurationMainService } from '../../electron-main/configurationMainService.js';
import { SystemSettings } from '../../electron-main/systemSettings.js';

/** A log that keeps its warnings and errors. */
class TestLogService extends NullLogService {
	public readonly warnings: unknown[][] = [];
	public readonly errors: unknown[][] = [];

	public override warn(...args: unknown[]): void {
		this.warnings.push(args);
	}

	public override error(...args: unknown[]): void {
		this.errors.push(args);
	}
}

/** A launch that cannot register a login item, on a desktop that shows tray icons. */
function systemSettings(): SystemSettings {
	const loginItem = { supported: false, read: () => Promise.resolve(false), write: () => Promise.resolve() };
	return new SystemSettings(loginItem, () => Promise.resolve(true));
}

suite('ConfigurationMainService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let folder: string;
	let path: string;
	let engine: TestEngineMainService;
	let log: TestLogService;

	/** The service on settings.json at `path`, read as main reads it at startup. */
	async function createService(): Promise<ConfigurationMainService> {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineMainService, engine);
		instantiationService.stub(ILogService, log);
		const engineMainService = instantiationService.get(IEngineMainService);
		const service = disposables.add(new ConfigurationMainService(path, engineMainService, instantiationService.get(ILogService), systemSettings()));
		await service.initialize();
		return service;
	}

	/** settings.json becomes a folder, so the next save fails; `allowSaves` undoes it. */
	async function refuseSaves(): Promise<void> {
		await mkdir(join(path, 'blocked'), { recursive: true });
	}
	async function allowSaves(): Promise<void> {
		await rm(path, { recursive: true, force: true });
	}

	/** The commands of `type` the engine was asked to run. */
	function sent<T extends EngineCommand['type']>(type: T): Extract<EngineCommand, { type: T }>[] {
		return engine.commands.filter((command): command is Extract<EngineCommand, { type: T }> => command.type === type);
	}

	/** An engine that refuses the commands `refuses` names with `message`, and runs the rest. */
	function refusing(refuses: (command: EngineCommand) => boolean, message: string): (command: EngineCommand) => EngineReply {
		return command => {
			if (refuses(command)) {
				throw new Error(message);
			}
			return { type: 'done' };
		};
	}
	setup(async () => {
		folder = await mkdtemp(join(tmpdir(), 'vnilink-settings-'));
		path = join(folder, 'settings.json');
		engine = disposables.add(new TestEngineMainService(() => ({ type: 'done' })));
		log = new TestLogService();
	});
	teardown(async () => {
		await rm(folder, { recursive: true, force: true });
	});
	suite('user settings', () => {
		test('a change the engine refuses is not saved', async () => {
			engine.handler = refusing(() => true, 'Held keys');
			const configuration = await createService();
			await assert.rejects(configuration.updateValue('input.keyboardMode', 'Map'), /Held keys/);
			await assert.rejects(stat(path), { code: 'ENOENT' });
		});
		test('a change the disk refuses is undone in the engine', async () => {
			const configuration = await createService();
			await refuseSaves();
			await assert.rejects(configuration.updateValue('input.keyboardMode', 'Map'));
			assert.strictEqual(sent('setInputDefaults').at(-1)?.mode, 'Character');
			assert.strictEqual(configuration.snapshot.values['input.keyboardMode'], 'Character');
		});
		test('a saved change reaches the engine with the rest of its group', async () => {
			const configuration = await createService();
			await configuration.updateValue('input.keyboardMode', 'Map');
			assert.strictEqual(configuration.snapshot.values['input.keyboardMode'], 'Map');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setInputDefaults', mode: 'Map', swap: false, pointer: { sensitivity_percent: 100, reverse_wheel: false, swap_buttons: false } });
		});
		test('a codec change the disk refuses gives the engine back its codec', async () => {
			const configuration = await createService();
			await refuseSaves();
			await assert.rejects(configuration.updateValue('desktop.codec', 'av1'));
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setDesktopCodecDefault', codec: 'auto' });
			await allowSaves();
			await configuration.updateValue('desktop.codec', 'av1');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setDesktopCodecDefault', codec: 'av1' });
		});
		test('a codec that is not offered is refused', async () => {
			const configuration = await createService();
			await assert.rejects(configuration.updateValue('desktop.codec', 'bogus'), /available options/);
		});
		test('an unknown setting is refused', async () => {
			const configuration = await createService();
			await assert.rejects(configuration.updateValue('unknown.setting', true), /Unknown setting/);
		});
	});
	suite('media sources', () => {
		test('a microphone the engine cannot open is not saved, and the engine is given the default sources back', async () => {
			let refused = false;
			engine.handler = command => {
				if (command.type === 'setNativeAudioSources' && command.microphones.includes('mic') && !refused) {
					refused = true;
					throw new Error('Audio source unavailable');
				}
				return { type: 'done' };
			};
			const configuration = await createService();
			await assert.rejects(configuration.updateValue('media.microphone', 'mic'), /Audio source unavailable/);
			assert.strictEqual(configuration.snapshot.values['media.microphone'], 'vnilink:default-microphone');
			assert.deepStrictEqual(engine.commands.map(command => command.type), ['setNativeAudioSources', 'setNativeAudioSources']);
			assert.deepStrictEqual(engine.commands[1], { type: 'setNativeAudioSources', microphones: ['vnilink:default-microphone'], system: ['vnilink:default-system-audio'] });
		});
	});
	suite('terminal shell', () => {
		test('the offered shell is chosen by its id', async () => {
			const configuration = await createService();
			await configuration.updateValue('terminal.shell', '/bin/zsh');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setTerminalShell', shell: '/bin/zsh' });
			assert.strictEqual(configuration.snapshot.values['terminal.shell'], '/bin/zsh');
		});
		test('a shell change while hosted shells run is refused with what to do, and nothing is saved', async () => {
			const configuration = await createService();
			await configuration.updateValue('terminal.shell', '/bin/zsh');
			engine.handler = refusing(command => command.type === 'setTerminalShell', 'Busy: the host already has a controlling session');
			await assert.rejects(configuration.updateValue('terminal.shell', 'git-bash'), /Close hosted terminals before changing the shell/);
			assert.strictEqual(configuration.snapshot.values['terminal.shell'], '/bin/zsh');
		});
		test('a shell that is not installed is refused', async () => {
			engine.handler = refusing(command => command.type === 'setTerminalShell', 'InvalidSetting: that shell is not installed on this computer');
			const configuration = await createService();
			await assert.rejects(configuration.updateValue('terminal.shell', 'gone'), /not installed/);
		});
		test('the automatic shell is sent as no shell', async () => {
			const configuration = await createService();
			await configuration.updateValue('terminal.shell', '/bin/zsh');
			await configuration.updateValue('terminal.shell', '');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setTerminalShell', shell: null });
		});
		test('a shell id with a NUL character is refused', async () => {
			const configuration = await createService();
			await assert.rejects(configuration.updateValue('terminal.shell', 'a\0b'), /installed on this computer/);
		});
		test('a saved shell is offered again when the engine starts', async () => {
			await writeFile(path, JSON.stringify({ 'terminal.shell': 'fish' }));
			const configuration = await createService();
			await configuration.restoreEngineDefaults(false);
			assert.deepStrictEqual(sent('setTerminalShell'), [{ type: 'setTerminalShell', shell: 'fish' }]);
		});
		test('a saved shell no longer installed leaves the automatic shell, is logged, and stays saved for when it returns', async () => {
			await writeFile(path, JSON.stringify({ 'terminal.shell': 'fish' }));
			engine.handler = refusing(command => command.type === 'setTerminalShell', 'InvalidSetting: that shell is not installed on this computer');
			const configuration = await createService();
			await configuration.restoreEngineDefaults(false);
			assert.strictEqual(log.warnings.length, 1);
			assert.strictEqual(configuration.snapshot.values['terminal.shell'], 'fish');
		});
		test('a temporary engine is given no host shell', async () => {
			await writeFile(path, JSON.stringify({ 'terminal.shell': 'fish' }));
			const configuration = await createService();
			await configuration.restoreEngineDefaults(true);
			assert.deepStrictEqual(sent('setTerminalShell'), []);
		});
	});
	suite('engine log', () => {
		/** An engine that refuses the filter `vnilink=loud` and reports every other one it is given. */
		function logFilters(command: EngineCommand): EngineReply {
			if (command.type === 'setLogFilter' && command.directives === 'vnilink=loud') {
				throw new Error('InvalidArg: The log filter is not valid');
			}
			const directives = command.type === 'setLogFilter' ? command.directives : null;
			return { type: 'logFilter', directives: directives ?? 'warn,vnilink=info', source: directives === null ? 'default' : 'setting' };
		}
		setup(() => {
			engine.handler = logFilters;
		});
		test('a level sends libraries\' warnings and that level for VniLink', async () => {
			const configuration = await createService();
			await configuration.updateValue('diagnostics.engineLog', 'debug');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setLogFilter', directives: 'warn,vnilink=debug' });
		});
		test('the log turned off sends off', async () => {
			const configuration = await createService();
			await configuration.updateValue('diagnostics.engineLog', 'off');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setLogFilter', directives: 'off' });
		});
		test('the advanced filter is sent trimmed, ahead of the level', async () => {
			const configuration = await createService();
			await configuration.updateValue('diagnostics.engineLog', 'off');
			await configuration.updateValue('diagnostics.engineLogFilter', ' vnilink::files=trace ');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setLogFilter', directives: 'vnilink::files=trace' });
		});
		test('a filter the engine refuses is not saved, and the previous one is sent again', async () => {
			const configuration = await createService();
			await configuration.updateValue('diagnostics.engineLogFilter', ' vnilink::files=trace ');
			await assert.rejects(configuration.updateValue('diagnostics.engineLogFilter', 'vnilink=loud'), /not valid/);
			assert.strictEqual(configuration.snapshot.values['diagnostics.engineLogFilter'], ' vnilink::files=trace ');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setLogFilter', directives: 'vnilink::files=trace' });
		});
		test('the default level is sent as the engine\'s default', async () => {
			const configuration = await createService();
			await configuration.updateValue('diagnostics.engineLog', 'debug');
			await configuration.updateValue('diagnostics.engineLog', 'info');
			assert.deepStrictEqual(engine.commands.at(-1), { type: 'setLogFilter', directives: null });
		});
		test('a filter longer than the engine takes is refused', async () => {
			const configuration = await createService();
			await assert.rejects(configuration.updateValue('diagnostics.engineLogFilter', 'x'.repeat(1025)), /shorter filter/);
		});
		test('a saved filter the engine refuses when it starts leaves its default and is logged', async () => {
			await writeFile(path, JSON.stringify({ 'diagnostics.engineLogFilter': 'vnilink=loud' }));
			const configuration = await createService();
			await configuration.restoreEngineDefaults(false);
			assert.deepStrictEqual(sent('setLogFilter'), [{ type: 'setLogFilter', directives: 'vnilink=loud' }]);
			assert.strictEqual(log.errors.length, 1);
		});
	});
});
