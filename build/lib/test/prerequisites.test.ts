// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { formatDoctor } from '../doctor.ts';
import {
	assertTaskReady, checkRequirement, checkTasks, distributionOf, firstBuildDownloads, fixLines, installCommand, latencyPortalGrants, linuxSandbox,
	linuxSandboxProfile, loadManifest, rustChannel, systemPackages, taskRequirements, type CheckResult, type Host, type Platform, type Probe, type TaskReport,
} from '../prerequisites.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));

const manifest = await loadManifest();

/** A computer described by data: files that exist, file contents and the output of each `command args` that succeeds. */
interface Computer {
	readonly files?: readonly string[];
	readonly contents?: Readonly<Record<string, string>>;
	readonly runs?: Readonly<Record<string, string>>;
}

function fakeProbe({ files = [], contents = {}, runs = {} }: Computer = {}): Probe {
	const present = new Set(files);
	return {
		run: (command, args) => {
			const key = [command, ...args].join(' ');
			return Promise.resolve(Object.hasOwn(runs, key) ? { ok: true, stdout: runs[key] } : { ok: false, stdout: '' });
		},
		exists: path => Promise.resolve(present.has(path) || Object.hasOwn(contents, path)),
		readFile: path => Promise.resolve(Object.hasOwn(contents, path) ? contents[path] : undefined),
		readdir: () => Promise.resolve([]),
		stat: () => Promise.resolve(undefined),
	};
}

function host(overrides: Partial<Host> = {}): Host {
	return { root: '/repo', platform: 'linux', architecture: 'x64', hardwareArchitecture: 'x64', nodeVersion: '22.19.0', osName: 'Ubuntu 26.04 LTS', distribution: 'debian', env: { PATH: '/usr/bin' }, ...overrides };
}

function windows(env: NodeJS.ProcessEnv): Host {
	return host({ platform: 'win32', distribution: undefined, env });
}

const mac = host({ platform: 'darwin', architecture: 'arm64', hardwareArchitecture: 'arm64', osName: 'macOS 27.0', distribution: undefined });

const repoFiles = { '/repo/.nvmrc': '22.19.0\n', '/repo/rust-toolchain.toml': '[toolchain]\nchannel = "1.91.0"\n' };

/** A Mac with everything the build needs, so a task's own requirements decide its status. */
function toolsMac(): Probe {
	return fakeProbe({
		contents: repoFiles,
		files: ['/usr/bin/rustup', '/usr/bin/git', '/usr/bin/curl', '/usr/bin/cmake', '/repo/node_modules/@vnilink/elg/prebuilds/darwin-arm64/node.napi.node'],
		runs: { 'rustup toolchain list': '1.91.0-aarch64-apple-darwin\n', 'xcode-select -p': '/Library/Developer/CommandLineTools\n', 'xcrun --find clang': '/usr/bin/clang\n' },
	});
}

function find<T>(items: readonly T[], predicate: (item: T) => boolean): T {
	const item = items.find(predicate);
	assert.ok(item !== undefined);
	return item;
}

function result(report: TaskReport, id: string): CheckResult {
	return find(report.results, candidate => candidate.id === id);
}

/** The run.ts task table, read from its source: importing it would run a task. */
async function runTasks(): Promise<string[]> {
	const source = await readFile(join(root, 'build/run.ts'), 'utf8');
	const table = source.slice(source.indexOf('const tasks'), source.indexOf('};', source.indexOf('const tasks')));
	return [...table.matchAll(/^\t'([^']+)':/gm)].map(match => match[1]);
}

const tasks = await runTasks();

const allPlatforms: readonly Platform[] = ['darwin', 'win32', 'linux'];

suite('prerequisites', () => {
	suite('manifest', () => {
		test('reads the task table from run.ts', () => {
			assert.ok(tasks.length > 20);
		});
		for (const task of tasks) {
			test(`declares the prerequisites of the run.ts task ${task}`, () => {
				assert.ok(Object.hasOwn(manifest.tasks, task));
			});
		}
		for (const [name, entry] of Object.entries(manifest.tasks)) {
			test(`${name} is a run.ts task or a command of its own`, () => {
				assert.ok(tasks.includes(name) || entry.run);
			});
			test(`${name} requires only declared requirements`, () => {
				assert.deepStrictEqual(taskRequirements(manifest, name).filter(id => !Object.hasOwn(manifest.requirements, id)), []);
			});
			test(`${name} has only known fields`, () => {
				const taskFields = ['description', 'run', 'platforms', 'requires', 'same'];
				assert.deepStrictEqual(Object.keys(entry).filter(field => !taskFields.includes(field)), []);
			});
		}
		test('declares no requirement that no task needs', () => {
			const used = new Set(Object.keys(manifest.tasks).flatMap(name => taskRequirements(manifest, name)));
			assert.deepStrictEqual(Object.keys(manifest.requirements).filter(id => !used.has(id)), []);
		});
		for (const [id, requirement] of Object.entries(manifest.requirements)) {
			for (const platform of requirement.platforms ?? allPlatforms) {
				test(`the ${id} check reaches a status on ${platform}`, async () => {
					const outcome = await checkRequirement(id, manifest, host({ platform, env: {} }), fakeProbe());
					assert.match(outcome.status, /^(ready|missing|unsupported|unknown)$/);
				});
			}
			test(`${id} has only known fields`, () => {
				const requirementFields = ['title', 'kind', 'purpose', 'platforms', 'optional', 'prepared', 'check', 'fix'];
				assert.deepStrictEqual(Object.keys(requirement).filter(field => !requirementFields.includes(field)), []);
			});
			test(`${id} is a tool, a permission or a computer`, () => {
				assert.match(requirement.kind ?? 'tool', /^(tool|permission|computer)$/);
			});
			if (requirement.kind === 'permission') {
				// A permission is the OS's: each names its OS and how to grant it.
				test(`the ${id} permission names its OS`, () => {
					assert.ok(requirement.platforms?.length);
				});
				test(`the ${id} permission says how to grant it`, () => {
					assert.ok(requirement.fix?.default);
				});
			}
			if (requirement.kind === 'computer') {
				test(`the ${id} computer is never required`, () => {
					assert.strictEqual(requirement.optional, true);
				});
			}
		}
		// What the runners call, by task.
		const needs: Readonly<Record<string, readonly string[]>> = {
			'check:hygiene': ['git'],
			'check:license': ['git'],
			'license:apply': ['git'],
			'vscode': ['git'],
			'measure:impaired:linux': ['root', 'network-namespaces'],
			'measure:latency': ['vnilink-screen-recording', 'screen-portal', 'signed-in-desktop'],
		};
		for (const [task, ids] of Object.entries(needs)) {
			test(`${task} declares ${ids.join(', ')}`, () => {
				const declared = taskRequirements(manifest, task);
				assert.deepStrictEqual(ids.filter(id => !declared.includes(id)), []);
			});
			test(`${task} cannot run without ${ids.join(', ')}`, () => {
				assert.deepStrictEqual(ids.filter(id => manifest.requirements[id].optional), []);
			});
		}
		test('declares no SSH requirement', () => {
			assert.strictEqual(Object.hasOwn(manifest.requirements, 'ssh'), false);
		});
		for (const task of ['test:service', 'bench:service', 'measure:latency']) {
			test(`${task} lists the second computer`, () => {
				assert.ok(taskRequirements(manifest, task).includes('second-computer'));
			});
		}
	});
	suite('Node', () => {
		const old = (): Promise<CheckResult> => checkRequirement('node', manifest, host({ nodeVersion: '22.12.0' }), fakeProbe({ contents: repoFiles }));
		const rosetta = (): Promise<CheckResult> => checkRequirement('node', manifest, host({ platform: 'darwin', architecture: 'x64', hardwareArchitecture: 'arm64' }), fakeProbe({ contents: repoFiles }));
		test('refuses a Node older than .nvmrc', async () => {
			assert.strictEqual((await old()).status, 'missing');
		});
		test('names the Node found and the versions .nvmrc accepts', async () => {
			assert.match((await old()).detail ?? '', /Node 22\.12\.0; needs 22\.19\.0 or a later 22\.x \(\.nvmrc\)/);
		});
		test('fixes an old Node with nvm', async () => {
			assert.match((await old()).fix.join('\n'), /nvm install/);
		});
		test('refuses x64 Node on an arm64 Mac', async () => {
			assert.strictEqual((await rosetta()).status, 'missing');
		});
		test('names the architecture of Node and of the computer', async () => {
			assert.match((await rosetta()).detail ?? '', /Node is x64 on an arm64 computer/);
		});
		test('fixes x64 Node on an arm64 Mac with the arm64 build of the pinned version', async () => {
			assert.match((await rosetta()).fix[0], /arm64 build of Node 22\.19\.0/);
		});
	});
	suite('platform', () => {
		const intelHost = host({ platform: 'darwin', architecture: 'x64', hardwareArchitecture: 'x64', osName: 'macOS 15.0' });
		const intel = (): Promise<TaskReport[]> => checkTasks(manifest, intelHost, fakeProbe({ contents: repoFiles }));
		// npm ci refuses to install; tasks that only run Node (formatting, checks) would still work.
		for (const name of Object.keys(manifest.tasks).filter(task => taskRequirements(manifest, task).includes('platform'))) {
			test(`${name} is unsupported on an Intel Mac`, async () => {
				assert.strictEqual(find(await intel(), report => report.name === name).status, 'unsupported');
			});
		}
		test('the install task, which runs npm ci, is unsupported on an Intel Mac', async () => {
			assert.strictEqual(find(await intel(), report => report.name === 'install').status, 'unsupported');
		});
		test('an Intel Mac is told the build needs Apple silicon', async () => {
			assert.match(find(await intel(), report => report.name === 'build').reason ?? '', /Intel Macs cannot build VniLink.*Apple silicon/);
		});
		test('install is unsupported on Windows on Arm', async () => {
			const [install] = await checkTasks(manifest, host({ platform: 'win32', architecture: 'arm64', hardwareArchitecture: 'arm64' }), fakeProbe(), ['install']);
			assert.strictEqual(install.status, 'unsupported');
		});
		const linuxArm = (): Promise<CheckResult> => checkRequirement('platform', manifest, host({ architecture: 'arm64', hardwareArchitecture: 'arm64' }), fakeProbe());
		test('Linux arm64 is ready', async () => {
			assert.strictEqual((await linuxArm()).status, 'ready');
		});
		test('Linux arm64 is warned that it has not run on an arm64 desktop', async () => {
			assert.match((await linuxArm()).note ?? '', /has not run on an arm64 desktop/);
		});
		const unknown = (): Promise<CheckResult> => checkRequirement('platform', manifest, host({ distribution: undefined, env: { XDG_CURRENT_DESKTOP: 'KDE' } }), fakeProbe());
		test('an unknown distribution and desktop are ready', async () => {
			assert.strictEqual((await unknown()).status, 'ready');
		});
		test('an unknown distribution is warned that it is unverified', async () => {
			assert.match((await unknown()).note ?? '', /distribution is unverified/);
		});
		test('a desktop other than GNOME is warned that only GNOME has been exercised', async () => {
			assert.match((await unknown()).note ?? '', /KDE: .*only GNOME's has been exercised/);
		});
		test('GNOME on a known distribution has no warning', async () => {
			assert.strictEqual((await checkRequirement('platform', manifest, host({ env: { XDG_CURRENT_DESKTOP: 'ubuntu:GNOME' } }), fakeProbe())).note, undefined);
		});
	});
	suite('distributions', () => {
		for (const [name, osRelease, distribution] of [
			['Ubuntu', 'ID=ubuntu\nID_LIKE=debian\n', 'debian'],
			['Linux Mint, by its ID_LIKE', 'ID=linuxmint\nID_LIKE="ubuntu debian"\n', 'debian'],
			['Debian', 'ID=debian\n', 'debian'],
			['Fedora', 'ID=fedora\n', 'fedora'],
		] as const) {
			test(`recognises ${name} as ${distribution}`, () => {
				assert.strictEqual(distributionOf(manifest, osRelease), distribution);
			});
		}
		test('does not recognise Arch', () => {
			assert.strictEqual(distributionOf(manifest, 'ID=arch\n'), undefined);
		});
	});
	suite('pkg-config modules', () => {
		const absent = ['libpipewire-0.3', 'libspa-0.2', 'libva', 'gbm'];
		const runs = Object.fromEntries(Object.keys(manifest.requirements['linux-build-libraries'].check['pkg-config'] ?? {}).filter(module => !absent.includes(module)).map(module => [`pkg-config --exists ${module}`, '']));
		const probe = fakeProbe({ files: ['/usr/bin/pkg-config'], runs });
		const check = (distribution: Host['distribution']): Promise<CheckResult> => checkRequirement('linux-build-libraries', manifest, host({ distribution }), probe);
		test('a missing module is missing', async () => {
			assert.strictEqual((await check('debian')).status, 'missing');
		});
		test('names the modules pkg-config cannot find', async () => {
			assert.strictEqual((await check('debian')).detail, 'pkg-config cannot find libpipewire-0.3, libspa-0.2, libva, gbm');
		});
		test('maps missing modules to the apt packages that provide them', async () => {
			assert.deepStrictEqual((await check('debian')).packages, ['libpipewire-0.3-dev', 'libspa-0.2-dev', 'libva-dev', 'libgbm-dev']);
		});
		test('installs the apt packages in one sorted command', async () => {
			assert.strictEqual(installCommand(manifest, host(), [await check('debian')]), 'sudo apt-get install --no-install-recommends libgbm-dev libpipewire-0.3-dev libspa-0.2-dev libva-dev');
		});
		test('maps missing modules to the dnf packages that provide them', async () => {
			assert.deepStrictEqual((await check('fedora')).packages, ['pipewire-devel', 'libva-devel', 'mesa-libgbm-devel']);
		});
		test('installs the dnf packages in one sorted command', async () => {
			const fedora = host({ distribution: 'fedora' });
			assert.strictEqual(installCommand(manifest, fedora, [await check('fedora')]), 'sudo dnf install libva-devel mesa-libgbm-devel pipewire-devel');
		});
		test('names no packages on another distribution', async () => {
			assert.deepStrictEqual((await check(undefined)).packages, []);
		});
		test('names the module and both families\' packages on another distribution', async () => {
			assert.ok((await check(undefined)).fix.includes('libva: the package providing it (Debian: libva-dev, Fedora: libva-devel)'));
		});
		test('reports every module missing without pkg-config', async () => {
			assert.strictEqual((await checkRequirement('linux-build-libraries', manifest, host(), fakeProbe())).status, 'missing');
		});
	});
	suite('commands', () => {
		const tools = manifest.requirements['linux-build-tools'].check.commands ?? {};
		const files = Object.keys(tools).filter(name => name !== 'nasm').map(name => `/usr/bin/${name}`);
		test('names a command missing from PATH', async () => {
			assert.strictEqual((await checkRequirement('linux-build-tools', manifest, host(), fakeProbe({ files }))).detail, 'nasm not found');
		});
		test('maps a missing command to its package', async () => {
			assert.deepStrictEqual((await checkRequirement('linux-build-tools', manifest, host(), fakeProbe({ files }))).packages, ['nasm']);
		});
		test('does not need nasm, which is only for x86 assembly, on arm64', async () => {
			const arm = await checkRequirement('linux-build-tools', manifest, host({ architecture: 'arm64', hardwareArchitecture: 'arm64' }), fakeProbe({ files }));
			assert.strictEqual(arm.status, 'ready');
		});
		test('finds commands in sbin on Linux', async () => {
			const apparmor = await checkRequirement('apparmor', manifest, host(), fakeProbe({ files: ['/sys/kernel/security/apparmor', '/usr/sbin/apparmor_parser'] }));
			assert.strictEqual(apparmor.status, 'ready');
		});
		const windowsHost = windows({ Path: 'C:\\Git\\cmd;C:\\Windows\\System32', PATHEXT: '.EXE' });
		const git = fakeProbe({ files: ['C:\\Git\\cmd\\git.EXE'] });
		test('names a command missing from the Windows Path', async () => {
			assert.strictEqual((await checkRequirement('curl', manifest, windowsHost, git)).detail, 'curl not found');
		});
		test('says curl is included in Windows', async () => {
			assert.deepStrictEqual((await checkRequirement('curl', manifest, windowsHost, git)).fix, ['included in Windows 10 and later']);
		});
		test('finds commands with a PATHEXT extension on Windows', async () => {
			assert.strictEqual((await checkRequirement('git', manifest, windowsHost, git)).status, 'ready');
		});
	});
	suite('shared libraries', () => {
		const libraries = Object.keys(manifest.requirements['linux-runtime-libraries'].check.libraries ?? {}).filter(name => name !== 'libsecret-1.so.0');
		const cache = libraries.map(name => `\t${name} (libc6,x86-64) => /usr/lib/x86_64-linux-gnu/${name}`).join('\n');
		const cached = (): Promise<CheckResult> => checkRequirement('linux-runtime-libraries', manifest, host(), fakeProbe({ files: ['/usr/bin/xdg-open'], runs: { '/sbin/ldconfig -p': cache } }));
		test('names a library missing from the linker cache', async () => {
			assert.strictEqual((await cached()).detail, 'libsecret-1.so.0 not found');
		});
		test('maps a missing library to its package', async () => {
			assert.deepStrictEqual((await cached()).packages, ['libsecret-1-0']);
		});
		test('is unknown without the linker cache', async () => {
			assert.strictEqual((await checkRequirement('linux-runtime-libraries', manifest, host(), fakeProbe({ files: ['/usr/bin/xdg-open'] }))).status, 'unknown');
		});
	});
	suite('Windows toolchain', () => {
		const tools = manifest.requirements.mingw.check.tools ?? [];
		const bin = 'D:\\msys64\\mingw64\\bin';
		const complete = (): Promise<CheckResult> => checkRequirement('mingw', manifest, windows({ Path: 'C:\\Windows', VNILINK_MSYS_ROOT: 'D:\\msys64' }), fakeProbe({ files: tools.map(tool => `${bin}\\${tool}`) }));
		const partial = (): Promise<CheckResult> => checkRequirement('mingw', manifest, windows({ Path: `${bin};C:\\Windows` }), fakeProbe({ files: [`${bin}\\dlltool.exe`, `${bin}\\gcc.exe`] }));
		test('finds MinGW in the MSYS2 root', async () => {
			assert.strictEqual((await complete()).status, 'ready');
		});
		test('names the MinGW directory found', async () => {
			assert.strictEqual((await complete()).detail, bin);
		});
		test('a MinGW on PATH lacking tools is missing', async () => {
			assert.strictEqual((await partial()).status, 'missing');
		});
		test('names the MinGW tools lacking', async () => {
			assert.match((await partial()).detail ?? '', /lacks clang\.exe, libclang\.dll, nasm\.exe, mingw32-make\.exe/);
		});
		test('installs the lacking MinGW tools with pacman', async () => {
			assert.match((await partial()).fix[0], /pacman -S --needed mingw-w64-x86_64-gcc mingw-w64-x86_64-clang mingw-w64-x86_64-nasm mingw-w64-x86_64-make/);
		});
		test('names where MinGW was looked for when there is none', async () => {
			assert.match((await checkRequirement('mingw', manifest, windows({ Path: 'C:\\Windows' }), fakeProbe())).detail ?? '', /no MinGW on PATH or in C:\\msys64/);
		});
		const rustupShow = (defaultHost: string, studio?: string): Probe => {
			const vswhere = 'C:\\Program Files (x86)\\Microsoft Visual Studio\\Installer\\vswhere.exe -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath';
			return fakeProbe({ runs: { 'rustup show': `Default host: ${defaultHost}\n`, ...(studio === undefined ? {} : { [vswhere]: studio }) } });
		};
		test('a GNU Rust host is ready', async () => {
			assert.strictEqual((await checkRequirement('rust-windows-host', manifest, windows({}), rustupShow('x86_64-pc-windows-gnu'))).status, 'ready');
		});
		test('an MSVC Rust host without Visual Studio is missing', async () => {
			assert.strictEqual((await checkRequirement('rust-windows-host', manifest, windows({}), rustupShow('x86_64-pc-windows-msvc'))).status, 'missing');
		});
		test('fixes an MSVC Rust host by making GNU the default', async () => {
			assert.match((await checkRequirement('rust-windows-host', manifest, windows({}), rustupShow('x86_64-pc-windows-msvc'))).fix[0], /rustup set default-host x86_64-pc-windows-gnu/);
		});
		test('an MSVC Rust host with Visual Studio is ready', async () => {
			assert.strictEqual((await checkRequirement('rust-windows-host', manifest, windows({}), rustupShow('x86_64-pc-windows-msvc', 'C:\\VS\n'))).status, 'ready');
		});
		const target = (): Promise<CheckResult> => checkRequirement('rust-windows-target', manifest, windows({}), fakeProbe({ contents: repoFiles, runs: { 'rustup target list --installed --toolchain 1.91.0': 'x86_64-pc-windows-msvc\n' } }));
		test('the GNU Rust target missing from the pinned toolchain is missing', async () => {
			assert.strictEqual((await target()).status, 'missing');
		});
		test('fixes the GNU Rust target with rustup', async () => {
			assert.match((await target()).fix[0], /rustup target add x86_64-pc-windows-gnu/);
		});
	});
	suite('Rust', () => {
		const none = (): Promise<CheckResult> => checkRequirement('rust', manifest, host(), fakeProbe({ contents: repoFiles }));
		const withToolchains = (list: string): Promise<CheckResult> => checkRequirement('rust', manifest, host(), fakeProbe({ contents: repoFiles, files: ['/usr/bin/rustup'], runs: { 'rustup toolchain list': list } }));
		test('says when rustup is not installed', async () => {
			assert.strictEqual((await none()).detail, 'rustup is not installed');
		});
		test('fixes a missing rustup from rustup.rs', async () => {
			assert.match((await none()).fix[0], /rustup\.rs/);
		});
		test('names the toolchain rust-toolchain.toml pins when it is not installed', async () => {
			assert.strictEqual((await withToolchains('stable-x86_64-unknown-linux-gnu (default)\n')).detail, 'Rust 1.91.0 (rust-toolchain.toml) is not installed');
		});
		test('installs the pinned toolchain with rustup', async () => {
			assert.deepStrictEqual((await withToolchains('stable-x86_64-unknown-linux-gnu (default)\n')).fix, ['`rustup toolchain install`']);
		});
		test('is ready with the pinned toolchain', async () => {
			assert.strictEqual((await withToolchains('stable-x86_64-unknown-linux-gnu\n1.91.0-x86_64-unknown-linux-gnu (active)\n')).status, 'ready');
		});
		test('reads the channel rust-toolchain.toml pins', async () => {
			assert.strictEqual(rustChannel(await readFile(join(root, 'rust-toolchain.toml'), 'utf8')), '1.91.0');
		});
		test('reads a nightly channel', () => {
			assert.strictEqual(rustChannel('[toolchain]\nchannel = "nightly-2026-01-01"\n'), 'nightly-2026-01-01');
		});
		test('builds Windows with the GNU target', () => {
			assert.strictEqual(manifest.platforms.win32.rustTarget, 'x86_64-pc-windows-gnu');
		});
	});
	suite('Chromium sandbox', () => {
		const restricted = { '/proc/sys/kernel/apparmor_restrict_unprivileged_userns': '1\n', '/repo/product.json': JSON.stringify({ platforms: { linux: { executableName: 'vnilink' } } }) };
		test('is ready when AppArmor does not restrict user namespaces', async () => {
			assert.strictEqual((await linuxSandbox('/repo', fakeProbe({ contents: { '/proc/sys/kernel/apparmor_restrict_unprivileged_userns': '0\n' } }), {})).status, 'ready');
		});
		test('is missing when AppArmor restricts user namespaces and no profile is installed', async () => {
			assert.strictEqual((await linuxSandbox('/repo', fakeProbe({ contents: restricted }), {})).status, 'missing');
		});
		test('is ready in the test container', async () => {
			assert.strictEqual((await linuxSandbox('/repo', fakeProbe({ contents: restricted }), { VNILINK_TEST_CONTAINER: '1' })).status, 'ready');
		});
		test('names the profile after the checkout', () => {
			assert.match(linuxSandboxProfile('/repo'), /^vnilink-development-[0-9a-f]{12}$/);
		});
		test('gives each checkout its own profile', () => {
			assert.notStrictEqual(linuxSandboxProfile('/repo'), linuxSandboxProfile('/other'));
		});
		test('is ready with the checkout\'s profile installed', async () => {
			assert.strictEqual((await linuxSandbox('/repo', fakeProbe({ contents: restricted, files: [`/etc/apparmor.d/${linuxSandboxProfile('/repo')}`] }), {})).status, 'ready');
		});
		// A prepared executable is asked for its AppArmor label, which a profile changes.
		const executable = '/repo/.build/electron/vnilink';
		const label = `${executable} -e process.stdout.write(require('node:fs').readFileSync('/proc/self/attr/current', 'utf8'))`;
		test('is missing when the prepared executable runs unconfined', async () => {
			assert.strictEqual((await linuxSandbox('/repo', fakeProbe({ contents: restricted, files: [executable], runs: { [label]: 'unconfined' } }), {})).status, 'missing');
		});
		test('is ready when the prepared executable runs under the development profile', async () => {
			assert.strictEqual((await linuxSandbox('/repo', fakeProbe({ contents: restricted, files: [executable], runs: { [label]: 'vnilink-development-0123 (unconfined)' } }), {})).status, 'ready');
		});
		// Without AppArmor, installing a profile is not supported, and launching needs none.
		test('installing the profile is unsupported without AppArmor', async () => {
			const [sandbox] = await checkTasks(manifest, host({ distribution: 'fedora' }), fakeProbe(), ['sandbox:linux']);
			assert.strictEqual(sandbox.status, 'unsupported');
		});
		test('says AppArmor is not active', async () => {
			const [sandbox] = await checkTasks(manifest, host({ distribution: 'fedora' }), fakeProbe(), ['sandbox:linux']);
			assert.match(sandbox.reason ?? '', /AppArmor is not active/);
		});
		test('installing the profile is for Linux only', async () => {
			const [sandbox] = await checkTasks(manifest, mac, fakeProbe(), ['sandbox:linux']);
			assert.strictEqual(sandbox.reason, 'Linux only');
		});
	});
	suite('UI test session', () => {
		const tools = Object.keys(manifest.requirements['test-display'].check.commands ?? {});
		test('names the display it uses', async () => {
			assert.strictEqual((await checkRequirement('test-display', manifest, host({ env: { DISPLAY: ':0' } }), fakeProbe())).detail, 'display :0');
		});
		const headless = (): Promise<CheckResult> => checkRequirement('test-display', manifest, host(), fakeProbe({ files: tools.map(tool => `/usr/bin/${tool}`) }));
		test('is ready without a display when the headless tools are installed', async () => {
			assert.strictEqual((await headless()).status, 'ready');
		});
		test('says scripts/test.sh runs the tests headless', async () => {
			assert.match((await headless()).note ?? '', /scripts\/test\.sh/);
		});
		const withoutXvfb = (): Promise<CheckResult> => checkRequirement('test-display', manifest, host(), fakeProbe({ files: tools.filter(tool => tool !== 'xvfb-run').map(tool => `/usr/bin/${tool}`) }));
		test('is missing without a display or xvfb-run', async () => {
			assert.strictEqual((await withoutXvfb()).status, 'missing');
		});
		test('installs xvfb for xvfb-run', async () => {
			assert.deepStrictEqual((await withoutXvfb()).packages, ['xvfb']);
		});
		const busctl = 'busctl --user --no-pager --no-legend list';
		const session = (): Promise<CheckResult> => checkRequirement('secret-service', manifest, host({ env: {} }), fakeProbe({ runs: { [busctl]: 'org.freedesktop.DBus 1 dbus-daemon\n' } }));
		test('the secret service is missing from a session bus without it', async () => {
			assert.strictEqual((await session()).status, 'missing');
		});
		test('installs gnome-keyring for the secret service', async () => {
			assert.deepStrictEqual((await session()).packages, ['gnome-keyring']);
		});
		test('the secret service is ready when the session bus can activate it', async () => {
			const keyring = await checkRequirement('secret-service', manifest, host({ env: {} }), fakeProbe({ runs: { [busctl]: 'org.freedesktop.secrets - - - (activatable) - -\n' } }));
			assert.strictEqual(keyring.status, 'ready');
		});
	});
	suite('task reports', () => {
		const linuxBuild = async (): Promise<TaskReport> => {
			const probe = fakeProbe({ contents: repoFiles, files: ['/usr/bin/rustup', '/usr/bin/git', '/usr/bin/curl', '/usr/bin/cmake'], runs: { 'rustup toolchain list': '1.91.0-x86_64-unknown-linux-gnu\n' } });
			const [build] = await checkTasks(manifest, host(), probe, ['build']);
			return build;
		};
		const missingFixLines = async (): Promise<string[]> => fixLines(manifest, host(), (await linuxBuild()).results.filter(outcome => outcome.status === 'missing'));
		test('a task missing a requirement is missing', async () => {
			assert.strictEqual((await linuxBuild()).status, 'missing');
		});
		test('reports each missing requirement once', async () => {
			assert.deepStrictEqual((await linuxBuild()).results.filter(outcome => outcome.status === 'missing').map(outcome => outcome.id), ['linux-build-tools', 'libclang', 'linux-build-libraries']);
		});
		test('fixes every missing package with one install command', async () => {
			assert.strictEqual((await missingFixLines()).filter(line => line.includes('sudo apt-get install')).length, 1);
		});
		test('ends with the install command for every missing package', async () => {
			assert.match((await missingFixLines()).at(-1) ?? '', /^ {4}sudo apt-get install --no-install-recommends .*build-essential.*libclang-dev.*libpipewire-0\.3-dev/);
		});
		const start = async (): Promise<TaskReport> => {
			const [report] = await checkTasks(manifest, mac, toolsMac(), ['start']);
			return report;
		};
		test('an optional requirement does not block a task', async () => {
			assert.strictEqual((await start()).status, 'ready');
		});
		test('reports a missing optional requirement as missing', async () => {
			assert.strictEqual(result(await start(), 'xcode').status, 'missing');
		});
		test('the doctor marks a missing optional requirement optional', async () => {
			assert.match(formatDoctor([await start()], 'header', [], []), /optional {4}Xcode \(actool\)/);
		});
		test('the doctor lists a ready task as ready', async () => {
			assert.match(formatDoctor([await start()], 'header', [], []), /ready {7}start/);
		});
	});
	suite('preflight', () => {
		let saved: string | undefined;
		beforeEach(() => {
			saved = process.env.VNILINK_SKIP_PREFLIGHT;
			delete process.env.VNILINK_SKIP_PREFLIGHT;
		});
		afterEach(() => {
			if (saved === undefined) {
				delete process.env.VNILINK_SKIP_PREFLIGHT;
			} else {
				process.env.VNILINK_SKIP_PREFLIGHT = saved;
			}
		});
		// This computer as it is, but for the files and commands given.
		const runs = { '/usr/sbin/sysctl -n hw.optional.arm64': process.arch === 'arm64' ? '1' : '0', '/usr/bin/sw_vers -productVersion': '27.0' };
		const pinning = (version: string): Probe => fakeProbe({ contents: { [join(root, '.nvmrc')]: version, '/etc/os-release': 'ID=ubuntu\n' }, runs });
		const refusal = async (): Promise<string> => {
			const error: unknown = await assertTaskReady(root, 'format', pinning('99.0.0')).then(() => undefined, (failure: unknown) => failure);
			assert.ok(error instanceof Error);
			return error.message;
		};
		test('a task whose requirements are met runs', async () => {
			await assertTaskReady(root, 'format', pinning(process.versions.node));
		});
		test('a task missing a requirement is refused, naming what is missing', async () => {
			assert.match(await refusal(), /^format needs what this computer is missing:\n {2}Node\.js: Node .*; needs 99\.0\.0/);
		});
		test('the refusal names the doctor and how to run the task anyway', async () => {
			assert.match(await refusal(), /npm run doctor` reports every task; VNILINK_SKIP_PREFLIGHT=1 runs it anyway/);
		});
		test('VNILINK_SKIP_PREFLIGHT runs a task anyway', async () => {
			process.env.VNILINK_SKIP_PREFLIGHT = '1';
			await assertTaskReady(root, 'format', pinning('99.0.0'));
		});
	});
	suite('downloads', () => {
		test('the first native build announces each download', async () => {
			const fresh = await firstBuildDownloads(manifest, 'linux', '/repo', '/home/someone', fakeProbe());
			assert.deepStrictEqual(fresh.map(download => download.into), ['~/.cache/shiguredo_cmake', 'target/electron']);
		});
		test('the first native build announces no download already done', async () => {
			const cached = await firstBuildDownloads(manifest, 'linux', '/repo', '/home/someone', fakeProbe({ files: ['/home/someone/.cache/shiguredo_cmake', '/repo/target/electron'] }));
			assert.deepStrictEqual(cached, []);
		});
	});
	suite('package lists', () => {
		const containerTasks = ['build', 'test:ui', 'test:rust', 'measure:impaired:linux'];
		test('arm64 needs no nasm', () => {
			assert.ok(!systemPackages(manifest, 'debian', ['build'], 'arm64').includes('nasm'));
		});
		test('lists the MSYS2 packages of the Windows build', () => {
			assert.deepStrictEqual(systemPackages(manifest, 'msys2', ['build']), ['mingw-w64-x86_64-clang', 'mingw-w64-x86_64-cmake', 'mingw-w64-x86_64-gcc', 'mingw-w64-x86_64-make', 'mingw-w64-x86_64-nasm']);
		});
		test('a task that only runs Node needs no packages', () => {
			assert.deepStrictEqual(systemPackages(manifest, 'fedora', ['format']), []);
		});
		test('the Linux image installs the packages the manifest lists for its tasks', async () => {
			const dockerfile = await readFile(join(root, 'build/containers/Dockerfile.linux'), 'utf8');
			assert.match(dockerfile, new RegExp(`prerequisites\\.ts packages debian ${containerTasks.join(' ')}`));
		});
	});
	suite('permissions', () => {
		test('the development VniLink\'s Screen Recording grant is unknown until the run prompts for it', async () => {
			assert.strictEqual((await checkRequirement('vnilink-screen-recording', manifest, mac, fakeProbe())).status, 'unknown');
		});
		test('Screen Recording is a permission', async () => {
			assert.strictEqual((await checkRequirement('vnilink-screen-recording', manifest, mac, fakeProbe())).kind, 'permission');
		});
		test('says where to allow VniLink to record the screen', async () => {
			assert.match((await checkRequirement('vnilink-screen-recording', manifest, mac, fakeProbe())).fix[0], /allow VniLink in System Settings > Privacy & Security > Screen & System Audio Recording/i);
		});
		const [hostGrant, readerGrant] = latencyPortalGrants('/repo');
		test('keeps the portal answers in the checkout', () => {
			assert.deepStrictEqual([hostGrant, readerGrant], ['/repo/.build/latency/host-screen-grant', '/repo/.build/latency/reader-screen-grant']);
		});
		const portal = (files: readonly string[] = []): Promise<CheckResult> => checkRequirement('screen-portal', manifest, host(), fakeProbe({ files }));
		test('the screen portal is unknown before anyone answered it', async () => {
			assert.strictEqual((await portal()).status, 'unknown');
		});
		test('says the screen portal asks someone at this computer', async () => {
			assert.match((await portal()).detail ?? '', /asks someone at this computer/);
		});
		test('says to choose the display in both portal dialogs', async () => {
			assert.match((await portal()).fix[0], /choose the display in both portal dialogs/);
		});
		test('the screen portal is unknown with one answer of two', async () => {
			assert.strictEqual((await portal([hostGrant])).status, 'unknown');
		});
		test('the screen portal is ready with both answers', async () => {
			assert.strictEqual((await portal([hostGrant, readerGrant])).status, 'ready');
		});
		const desktop = (): Promise<CheckResult> => checkRequirement('signed-in-desktop', manifest, windows({ SESSIONNAME: 'Console' }), fakeProbe());
		const ssh = (): Promise<CheckResult> => checkRequirement('signed-in-desktop', manifest, windows({}), fakeProbe());
		test('a Windows console session is a signed-in desktop', async () => {
			assert.strictEqual((await desktop()).status, 'ready');
		});
		test('names the Windows session', async () => {
			assert.strictEqual((await desktop()).detail, 'session Console');
		});
		test('a Windows process without a session has no signed-in desktop', async () => {
			assert.strictEqual((await ssh()).status, 'missing');
		});
		test('says there is no signed-in desktop', async () => {
			assert.match((await ssh()).detail ?? '', /no signed-in desktop/);
		});
		test('says to run the task on the signed-in desktop', async () => {
			assert.deepStrictEqual((await ssh()).fix, ['Run the task from a terminal on the signed-in desktop.']);
		});
		test('root is ready as user 0', async () => {
			assert.strictEqual((await checkRequirement('root', manifest, host({ uid: 0 }), fakeProbe())).status, 'ready');
		});
		test('root is missing as another user', async () => {
			assert.strictEqual((await checkRequirement('root', manifest, host({ uid: 1000 }), fakeProbe())).status, 'missing');
		});
		test('says how to run the impaired measurement as root', async () => {
			assert.match((await checkRequirement('root', manifest, host({ uid: 1000 }), fakeProbe())).fix[0], /npm run measure:impaired/);
		});
		test('the impaired measurement needs root', async () => {
			const [impaired] = await checkTasks(manifest, host({ uid: 1000 }), fakeProbe(), ['measure:impaired:linux']);
			assert.match(impaired.reason ?? '', /Root/);
		});
	});
	suite('one computer', () => {
		const latency = async (): Promise<TaskReport> => {
			const [report] = await checkTasks(manifest, mac, toolsMac(), ['measure:latency']);
			return report;
		};
		const build = async (): Promise<TaskReport> => {
			const [report] = await checkTasks(manifest, mac, toolsMac(), ['build']);
			return report;
		};
		test('the latency harness on macOS is unknown but for the grant only its run can show', async () => {
			assert.strictEqual((await latency()).status, 'unknown');
		});
		test('names the grant that cannot be checked before the run', async () => {
			assert.strictEqual((await latency()).reason, 'cannot be checked before it runs: Screen Recording for the development VniLink');
		});
		test('reports a second computer as optional, and what --peer does with it', async () => {
			assert.deepStrictEqual(result(await latency(), 'second-computer'), {
				id: 'second-computer', title: 'A second computer', kind: 'computer', status: 'unknown', optional: true,
				detail: '--peer runs a suite or a measurement\'s other side against `vnilink-testkit agent` there, built from the same commit, once a person types this computer\'s digits on its terminal; every task runs on this computer without it', fix: [], packages: [],
			});
		});
		test('no task reaches another computer over SSH', async () => {
			assert.ok(!(await latency()).results.some(outcome => outcome.id === 'ssh'));
		});
		test('the build is ready on a Mac with the tools', async () => {
			assert.strictEqual((await build()).status, 'ready');
		});
		test('a ready task gives no reason', async () => {
			assert.strictEqual((await build()).reason, undefined);
		});
		test('the doctor lists a second computer as optional under other computers', async () => {
			const text = formatDoctor([await build(), await latency()], 'header', [], []);
			assert.match(text, /\n\nOther computers\n {2}optional {4}A second computer: --peer runs a suite or a measurement's other side/);
		});
		test('the doctor lists each task with its status and reason', async () => {
			const text = formatDoctor([await build(), await latency()], 'header', [], []);
			assert.match(text, /\nTasks\n {2}ready {7}build\n {2}unknown {5}measure:latency: cannot be checked before it runs: Screen Recording for the development VniLink$/m);
		});
	});
});
