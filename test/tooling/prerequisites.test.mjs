// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { formatDoctor } from '../../build/lib/doctor.ts';
import {
	assertTaskReady, checkRequirement, checkTasks, distributionOf, firstBuildDownloads, fixLines, installCommand, latencyPortalGrants, linuxSandbox,
	linuxSandboxProfile, loadManifest, rustChannel, systemPackages, taskRequirements,
} from '../../build/lib/prerequisites.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

const manifest = await loadManifest();


/**
 * A computer described by data: files that exist, file contents, commands on
 * PATH and the output of each `command args` it runs.
 */
function fakeProbe({ files = [], contents = {}, runs = {}, directories = {} } = {}) {
	const present = new Set(files);
	const ran = [];
	return {
		ran,
		run: async (command, args) => {
			const key = [command, ...args].join(' ');
			ran.push(key);
			const result = runs[key];
			return typeof result === 'string' ? { ok: true, stdout: result } : { ok: false, stdout: '' };
		},
		exists: async path => present.has(path) || Object.hasOwn(contents, path),
		readFile: async path => contents[path],
		readdir: async path => directories[path] ?? [],
		stat: async () => undefined,
	};
}

function host(overrides = {}) {
	return { root: '/repo', platform: 'linux', architecture: 'x64', hardwareArchitecture: 'x64', nodeVersion: '22.19.0', osName: 'Ubuntu 26.04 LTS', distribution: 'debian', env: { PATH: '/usr/bin' }, ...overrides };
}

const repoFiles = { '/repo/.nvmrc': '22.19.0\n', '/repo/rust-toolchain.toml': '[toolchain]\nchannel = "1.91.0"\n' };

/** The run.ts task table, read from its source: importing it would run a task. */
async function runTasks() {
	const source = await readFile(join(root, 'build/run.ts'), 'utf8');
	const table = source.slice(source.indexOf('const tasks'), source.indexOf('};', source.indexOf('const tasks')));
	return [...table.matchAll(/^\t'([^']+)':/gm)].map(match => match[1]);
}

test('every build task declares its prerequisites, and every declaration resolves', async () => {
	const tasks = await runTasks();
	assert(tasks.length > 20);
	for (const task of tasks) {
		assert(Object.hasOwn(manifest.tasks, task), `build/prerequisites.json has no entry for the run.ts task ${task}`);
	}
	for (const [name, entry] of Object.entries(manifest.tasks)) {
		assert(tasks.includes(name) || entry.run, `${name} is neither a run.ts task nor a command of its own`);
		for (const id of taskRequirements(manifest, name)) {
			assert(Object.hasOwn(manifest.requirements, id), `${name} requires the undeclared ${id}`);
		}
	}
	const used = new Set(Object.keys(manifest.tasks).flatMap(name => taskRequirements(manifest, name)));
	assert.deepEqual(Object.keys(manifest.requirements).filter(id => !used.has(id)), [], 'Requirements no task needs');
	// Every custom check has an implementation on the platforms it applies to.
	for (const [id, requirement] of Object.entries(manifest.requirements)) {
		for (const platform of requirement.platforms ?? ['darwin', 'win32', 'linux']) {
			const result = await checkRequirement(id, manifest, host({ platform, env: {} }), fakeProbe());
			assert.match(result.status, /^(ready|missing|unsupported|unknown)$/, `${id} on ${platform}`);
		}
	}
});

test('the Node check names the version and architecture it needs', async () => {
	const old = await checkRequirement('node', manifest, host({ nodeVersion: '22.12.0' }), fakeProbe({ contents: repoFiles }));
	assert.equal(old.status, 'missing');
	assert.match(old.detail, /Node 22\.12\.0; needs 22\.19\.0 or a later 22\.x \(\.nvmrc\)/);
	assert.match(old.fix.join('\n'), /nvm install/);
	const rosetta = await checkRequirement('node', manifest, host({ platform: 'darwin', architecture: 'x64', hardwareArchitecture: 'arm64' }), fakeProbe({ contents: repoFiles }));
	assert.equal(rosetta.status, 'missing');
	assert.match(rosetta.detail, /Node is x64 on an arm64 computer/);
	assert.match(rosetta.fix[0], /arm64 build of Node 22\.19\.0/);
});

test('Intel Macs and Windows on Arm are unsupported for every task; Linux arm64 runs with a warning', async () => {
	const intel = await checkTasks(manifest, host({ platform: 'darwin', architecture: 'x64', hardwareArchitecture: 'x64', osName: 'macOS 15.0' }), fakeProbe({ contents: repoFiles }));
	// npm ci refuses to install; tasks that only run Node (formatting, checks) would still work.
	for (const report of intel.filter(report => taskRequirements(manifest, report.name).includes('platform'))) {
		assert.equal(report.status, 'unsupported', report.name);
	}
	assert.equal(intel.find(report => report.name === 'install').status, 'unsupported');
	assert.match(intel.find(report => report.name === 'build').reason, /Intel Macs cannot build VniLink.*Apple silicon/);
	const arm = await checkTasks(manifest, host({ platform: 'win32', architecture: 'arm64', hardwareArchitecture: 'arm64' }), fakeProbe(), ['install']);
	assert.equal(arm[0].status, 'unsupported');
	const linuxArm = await checkRequirement('platform', manifest, host({ architecture: 'arm64', hardwareArchitecture: 'arm64' }), fakeProbe());
	assert.equal(linuxArm.status, 'ready');
	assert.match(linuxArm.note, /has not run on an arm64 desktop/);
	const unknown = await checkRequirement('platform', manifest, host({ distribution: undefined, env: { XDG_CURRENT_DESKTOP: 'KDE' } }), fakeProbe());
	assert.equal(unknown.status, 'ready');
	assert.match(unknown.note, /distribution is unverified/);
	assert.match(unknown.note, /KDE: .*only GNOME's has been exercised/);
	const gnome = await checkRequirement('platform', manifest, host({ env: { XDG_CURRENT_DESKTOP: 'ubuntu:GNOME' } }), fakeProbe());
	assert.equal(gnome.note, undefined);
});

test('distributions are recognised by ID and ID_LIKE', () => {
	assert.equal(distributionOf(manifest, 'ID=ubuntu\nID_LIKE=debian\n'), 'debian');
	assert.equal(distributionOf(manifest, 'ID=linuxmint\nID_LIKE="ubuntu debian"\n'), 'debian');
	assert.equal(distributionOf(manifest, 'ID=debian\n'), 'debian');
	assert.equal(distributionOf(manifest, 'ID=fedora\n'), 'fedora');
	assert.equal(distributionOf(manifest, 'ID=arch\n'), undefined);
});

test('missing pkg-config modules map to the apt or dnf packages that provide them', async () => {
	const runs = {};
	for (const module of Object.keys(manifest.requirements['linux-build-libraries'].check['pkg-config'])) {
		if (!['libpipewire-0.3', 'libspa-0.2', 'libva', 'gbm'].includes(module)) {
			runs[`pkg-config --exists ${module}`] = '';
		}
	}
	const probe = fakeProbe({ files: ['/usr/bin/pkg-config'], runs });
	const debian = await checkRequirement('linux-build-libraries', manifest, host(), probe);
	assert.equal(debian.status, 'missing');
	assert.equal(debian.detail, 'pkg-config cannot find libpipewire-0.3, libspa-0.2, libva, gbm');
	assert.deepEqual(debian.packages, ['libpipewire-0.3-dev', 'libspa-0.2-dev', 'libva-dev', 'libgbm-dev']);
	assert.equal(installCommand(manifest, host(), [debian]), 'sudo apt-get install --no-install-recommends libgbm-dev libpipewire-0.3-dev libspa-0.2-dev libva-dev');
	const fedora = await checkRequirement('linux-build-libraries', manifest, host({ distribution: 'fedora' }), probe);
	assert.deepEqual(fedora.packages, ['pipewire-devel', 'libva-devel', 'mesa-libgbm-devel']);
	assert.equal(installCommand(manifest, host({ distribution: 'fedora' }), [fedora]), 'sudo dnf install libva-devel mesa-libgbm-devel pipewire-devel');
	// Elsewhere the report names the module and both families' packages.
	const other = await checkRequirement('linux-build-libraries', manifest, host({ distribution: undefined }), probe);
	assert.deepEqual(other.packages, []);
	assert(other.fix.includes('libva: the package providing it (Debian: libva-dev, Fedora: libva-devel)'));
	// Without pkg-config every module is reported missing, not silently ready.
	const without = await checkRequirement('linux-build-libraries', manifest, host(), fakeProbe());
	assert.equal(without.status, 'missing');
});

test('commands are found on PATH, sbin included on Linux and PATHEXT on Windows', async () => {
	const tools = manifest.requirements['linux-build-tools'].check.commands;
	const files = Object.keys(tools).filter(name => name !== 'nasm').map(name => `/usr/bin/${name}`);
	const x64 = await checkRequirement('linux-build-tools', manifest, host(), fakeProbe({ files }));
	assert.equal(x64.detail, 'nasm not found');
	assert.deepEqual(x64.packages, ['nasm']);
	const arm = await checkRequirement('linux-build-tools', manifest, host({ architecture: 'arm64', hardwareArchitecture: 'arm64' }), fakeProbe({ files }));
	assert.equal(arm.status, 'ready', 'nasm is only for x86 assembly');
	const apparmor = await checkRequirement('apparmor', manifest, host(), fakeProbe({ files: ['/sys/kernel/security/apparmor', '/usr/sbin/apparmor_parser'] }));
	assert.equal(apparmor.status, 'ready');
	const windowsHost = host({ platform: 'win32', env: { Path: 'C:\\Git\\cmd;C:\\Windows\\System32', PATHEXT: '.EXE' } });
	const windows = await checkRequirement('curl', manifest, windowsHost, fakeProbe({ files: ['C:\\Git\\cmd\\git.EXE'] }));
	assert.equal(windows.detail, 'curl not found');
	assert.deepEqual(windows.fix, ['included in Windows 10 and later']);
	assert.equal((await checkRequirement('git', manifest, windowsHost, fakeProbe({ files: ['C:\\Git\\cmd\\git.EXE'] }))).status, 'ready');
});

test('shared libraries are read from the linker cache', async () => {
	const libraries = Object.keys(manifest.requirements['linux-runtime-libraries'].check.libraries).filter(name => name !== 'libsecret-1.so.0');
	const cache = libraries.map(name => `\t${name} (libc6,x86-64) => /usr/lib/x86_64-linux-gnu/${name}`).join('\n');
	const result = await checkRequirement('linux-runtime-libraries', manifest, host(), fakeProbe({ files: ['/usr/bin/xdg-open'], runs: { '/sbin/ldconfig -p': cache } }));
	assert.equal(result.detail, 'libsecret-1.so.0 not found');
	assert.deepEqual(result.packages, ['libsecret-1-0']);
	const unknown = await checkRequirement('linux-runtime-libraries', manifest, host(), fakeProbe({ files: ['/usr/bin/xdg-open'] }));
	assert.equal(unknown.status, 'unknown');
});

test('Windows needs MinGW from PATH or the MSYS2 root, and a GNU host or Visual Studio', async () => {
	const tools = manifest.requirements.mingw.check.tools;
	const bin = 'D:\\msys64\\mingw64\\bin';
	const windows = env => host({ platform: 'win32', distribution: undefined, env });
	const complete = await checkRequirement('mingw', manifest, windows({ Path: 'C:\\Windows', VNILINK_MSYS_ROOT: 'D:\\msys64' }), fakeProbe({ files: tools.map(tool => `${bin}\\${tool}`) }));
	assert.equal(complete.status, 'ready');
	assert.equal(complete.detail, bin);
	const partial = await checkRequirement('mingw', manifest, windows({ Path: `${bin};C:\\Windows` }), fakeProbe({ files: [`${bin}\\dlltool.exe`, `${bin}\\gcc.exe`] }));
	assert.equal(partial.status, 'missing');
	assert.match(partial.detail, /lacks clang\.exe, libclang\.dll, nasm\.exe, mingw32-make\.exe/);
	assert.match(partial.fix[0], /pacman -S --needed mingw-w64-x86_64-gcc mingw-w64-x86_64-clang mingw-w64-x86_64-nasm mingw-w64-x86_64-make/);
	const none = await checkRequirement('mingw', manifest, windows({ Path: 'C:\\Windows' }), fakeProbe());
	assert.match(none.detail, /no MinGW on PATH or in C:\\msys64/);
	const gnu = await checkRequirement('rust-windows-host', manifest, windows({}), fakeProbe({ runs: { 'rustup show': 'Default host: x86_64-pc-windows-gnu\n' } }));
	assert.equal(gnu.status, 'ready');
	const msvc = await checkRequirement('rust-windows-host', manifest, windows({}), fakeProbe({ runs: { 'rustup show': 'Default host: x86_64-pc-windows-msvc\n' } }));
	assert.equal(msvc.status, 'missing');
	assert.match(msvc.fix[0], /rustup set default-host x86_64-pc-windows-gnu/);
	const vswhere = 'C:\\Program Files (x86)\\Microsoft Visual Studio\\Installer\\vswhere.exe -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath';
	const studio = await checkRequirement('rust-windows-host', manifest, windows({}), fakeProbe({ runs: { 'rustup show': 'Default host: x86_64-pc-windows-msvc\n', [vswhere]: 'C:\\VS\n' } }));
	assert.equal(studio.status, 'ready');
	const target = await checkRequirement('rust-windows-target', manifest, windows({}), fakeProbe({ contents: repoFiles, runs: { 'rustup target list --installed --toolchain 1.91.0': 'x86_64-pc-windows-msvc\n' } }));
	assert.equal(target.status, 'missing');
	assert.match(target.fix[0], /rustup target add x86_64-pc-windows-gnu/);
});

test('Rust needs rustup and the toolchain rust-toolchain.toml names', async () => {
	const none = await checkRequirement('rust', manifest, host(), fakeProbe({ contents: repoFiles }));
	assert.equal(none.detail, 'rustup is not installed');
	assert.match(none.fix[0], /rustup\.rs/);
	const probe = runs => fakeProbe({ contents: repoFiles, files: ['/usr/bin/rustup'], runs });
	const stale = await checkRequirement('rust', manifest, host(), probe({ 'rustup toolchain list': 'stable-x86_64-unknown-linux-gnu (default)\n' }));
	assert.equal(stale.detail, 'Rust 1.91.0 (rust-toolchain.toml) is not installed');
	assert.deepEqual(stale.fix, ['`rustup toolchain install`']);
	const ready = await checkRequirement('rust', manifest, host(), probe({ 'rustup toolchain list': 'stable-x86_64-unknown-linux-gnu\n1.91.0-x86_64-unknown-linux-gnu (active)\n' }));
	assert.equal(ready.status, 'ready');
});

test('the Chromium sandbox check follows AppArmor, the setuid helper and the installed profile', async () => {
	const restricted = { '/proc/sys/kernel/apparmor_restrict_unprivileged_userns': '1\n', '/repo/product.json': JSON.stringify({ platforms: { linux: { executableName: 'vnilink' } } }) };
	assert.equal((await linuxSandbox('/repo', fakeProbe({ contents: { '/proc/sys/kernel/apparmor_restrict_unprivileged_userns': '0\n' } }), {})).status, 'ready');
	assert.equal((await linuxSandbox('/repo', fakeProbe({ contents: restricted }), {})).status, 'missing');
	assert.equal((await linuxSandbox('/repo', fakeProbe({ contents: restricted }), { VNILINK_TEST_CONTAINER: '1' })).status, 'ready');
	const profile = `/etc/apparmor.d/${linuxSandboxProfile('/repo')}`;
	assert.match(linuxSandboxProfile('/repo'), /^vnilink-development-[0-9a-f]{12}$/);
	assert.notEqual(linuxSandboxProfile('/repo'), linuxSandboxProfile('/other'));
	assert.equal((await linuxSandbox('/repo', fakeProbe({ contents: restricted, files: [profile] }), {})).status, 'ready');
	// A prepared executable is asked for its AppArmor label, which a profile changes.
	const executable = '/repo/.build/electron/vnilink';
	const label = `${executable} -e process.stdout.write(require('node:fs').readFileSync('/proc/self/attr/current', 'utf8'))`;
	assert.equal((await linuxSandbox('/repo', fakeProbe({ contents: restricted, files: [executable], runs: { [label]: 'unconfined' } }), {})).status, 'missing');
	assert.equal((await linuxSandbox('/repo', fakeProbe({ contents: restricted, files: [executable], runs: { [label]: 'vnilink-development-0123 (unconfined)' } }), {})).status, 'ready');
	// Without AppArmor, installing a profile is not supported, and launching needs none.
	const reports = await checkTasks(manifest, host({ distribution: 'fedora' }), fakeProbe(), ['sandbox:linux']);
	assert.equal(reports[0].status, 'unsupported');
	assert.match(reports[0].reason, /AppArmor is not active/);
	assert.equal((await checkTasks(manifest, host({ platform: 'darwin', hardwareArchitecture: 'arm64', architecture: 'arm64' }), fakeProbe(), ['sandbox:linux']))[0].reason, 'Linux only');
});

test('UI tests need a display, or the tools scripts/test.sh runs them headless with', async () => {
	const tools = Object.keys(manifest.requirements['test-display'].check.commands);
	const display = await checkRequirement('test-display', manifest, host({ env: { DISPLAY: ':0' } }), fakeProbe());
	assert.equal(display.detail, 'display :0');
	const headless = await checkRequirement('test-display', manifest, host(), fakeProbe({ files: tools.map(tool => `/usr/bin/${tool}`) }));
	assert.equal(headless.status, 'ready');
	assert.match(headless.note, /scripts\/test\.sh/);
	const missing = await checkRequirement('test-display', manifest, host(), fakeProbe({ files: tools.filter(tool => tool !== 'xvfb-run').map(tool => `/usr/bin/${tool}`) }));
	assert.equal(missing.status, 'missing');
	assert.deepEqual(missing.packages, ['xvfb']);
	const session = await checkRequirement('secret-service', manifest, host({ env: {} }), fakeProbe({ runs: { 'busctl --user --no-pager --no-legend list': 'org.freedesktop.DBus 1 dbus-daemon\n' } }));
	assert.equal(session.status, 'missing');
	assert.deepEqual(session.packages, ['gnome-keyring']);
	const keyring = await checkRequirement('secret-service', manifest, host({ env: {} }), fakeProbe({ runs: { 'busctl --user --no-pager --no-legend list': 'org.freedesktop.secrets - - - (activatable) - -\n' } }));
	assert.equal(keyring.status, 'ready');
});

test('a task reports every missing requirement once, with one install command, and optional ones do not block', async () => {
	const probe = fakeProbe({ contents: repoFiles, files: ['/usr/bin/rustup', '/usr/bin/git', '/usr/bin/curl', '/usr/bin/cmake'], runs: { 'rustup toolchain list': '1.91.0-x86_64-unknown-linux-gnu\n' } });
	const [build] = await checkTasks(manifest, host(), probe, ['build']);
	assert.equal(build.status, 'missing');
	assert.deepEqual(build.results.filter(result => result.status === 'missing').map(result => result.id), ['linux-build-tools', 'libclang', 'linux-build-libraries']);
	const lines = fixLines(manifest, host(), build.results.filter(result => result.status === 'missing'));
	assert.equal(lines.filter(line => line.includes('sudo apt-get install')).length, 1);
	assert.match(lines.at(-1), /^ {4}sudo apt-get install --no-install-recommends .*build-essential.*libclang-dev.*libpipewire-0\.3-dev/);
	const mac = host({ platform: 'darwin', hardwareArchitecture: 'arm64', architecture: 'arm64', distribution: undefined });
	const macProbe = fakeProbe({
		contents: repoFiles,
		files: ['/usr/bin/rustup', '/usr/bin/git', '/usr/bin/curl', '/repo/node_modules/@vnilink/elg/prebuilds/darwin-arm64/node.napi.node'],
		runs: { 'rustup toolchain list': '1.91.0-aarch64-apple-darwin\n', 'xcode-select -p': '/Library/Developer/CommandLineTools\n', 'xcrun --find clang': '/usr/bin/clang\n' },
	});
	const [start] = await checkTasks(manifest, { ...mac, env: { PATH: '/usr/bin' } }, macProbe, ['start']);
	assert.equal(start.status, 'ready', 'actool (full Xcode) is optional');
	assert.equal(start.results.find(result => result.id === 'xcode').status, 'missing');
	const text = formatDoctor([start], 'header', [], []);
	assert.match(text, /optional {4}Xcode \(actool\)/);
	assert.match(text, /ready {7}start/);
});

test('each task checks its own requirements before it runs, naming the fix and the doctor', async () => {
	const saved = process.env.VNILINK_SKIP_PREFLIGHT;
	delete process.env.VNILINK_SKIP_PREFLIGHT;
	try {
		// This computer as it is, but for the files and commands given.
		const runs = { '/usr/sbin/sysctl -n hw.optional.arm64': process.arch === 'arm64' ? '1' : '0', '/usr/bin/sw_vers -productVersion': '27.0' };
		const nvmrc = version => ({ [join(root, '.nvmrc')]: version, '/etc/os-release': 'ID=ubuntu\n' });
		await assertTaskReady(root, 'format', fakeProbe({ contents: nvmrc(process.versions.node), runs }));
		const probe = fakeProbe({ contents: nvmrc('99.0.0'), runs });
		await assert.rejects(assertTaskReady(root, 'format', probe), error => {
			assert.match(error.message, /^format needs what this computer is missing:\n {2}Node\.js: Node .*; needs 99\.0\.0/);
			assert.match(error.message, /npm run doctor` reports every task; VNILINK_SKIP_PREFLIGHT=1 runs it anyway/);
			return true;
		});
		process.env.VNILINK_SKIP_PREFLIGHT = '1';
		await assertTaskReady(root, 'format', probe);
	} finally {
		if (saved === undefined) {
			delete process.env.VNILINK_SKIP_PREFLIGHT;
		} else {
			process.env.VNILINK_SKIP_PREFLIGHT = saved;
		}
	}
});

test('the first native build announces only the downloads it has not done', async () => {
	const fresh = await firstBuildDownloads(manifest, 'linux', '/repo', '/home/someone', fakeProbe());
	assert.deepEqual(fresh.map(download => download.into), ['~/.cache/shiguredo_cmake', 'target/electron']);
	const cached = await firstBuildDownloads(manifest, 'linux', '/repo', '/home/someone', fakeProbe({ files: ['/home/someone/.cache/shiguredo_cmake', '/repo/target/electron'] }));
	assert.deepEqual(cached, []);
});

test('package lists for the container and CI come from the manifest', async () => {
	const container = systemPackages(manifest, 'debian', ['build', 'test:ui', 'test:rust', 'measure:impaired:linux']);
	for (const name of ['build-essential', 'cmake', 'nasm', 'libclang-dev', 'libpipewire-0.3-dev', 'libva-dev', 'libgbm-dev', 'libegl-dev', 'libdbus-1-dev', 'libasound2-dev', 'xvfb', 'openbox', 'x11-utils', 'gnome-keyring', 'dbus-x11', 'iproute2', 'libnss3']) {
		assert(container.includes(name), `${name} is missing from the container's packages`);
	}
	assert(!systemPackages(manifest, 'debian', ['build'], 'arm64').includes('nasm'));
	assert.deepEqual(systemPackages(manifest, 'msys2', ['build']), ['mingw-w64-x86_64-clang', 'mingw-w64-x86_64-cmake', 'mingw-w64-x86_64-gcc', 'mingw-w64-x86_64-make', 'mingw-w64-x86_64-nasm']);
	assert.deepEqual(systemPackages(manifest, 'fedora', ['format']), []);
	const dockerfile = await readFile(join(root, 'build/containers/Dockerfile.linux'), 'utf8');
	assert.match(dockerfile, /prerequisites\.ts packages debian build test:ui test:rust measure:impaired:linux/);
});

test('the Rust toolchain and the Windows target are named once', async () => {
	assert.equal(rustChannel(await readFile(join(root, 'rust-toolchain.toml'), 'utf8')), '1.91.0');
	assert.equal(rustChannel('[toolchain]\nchannel = "nightly-2026-01-01"\n'), 'nightly-2026-01-01');
	assert.equal(manifest.platforms.win32.rustTarget, 'x86_64-pc-windows-gnu');
});

test('prerequisites.ts imports only Node modules, so preinstall and the Linux image can run it alone', async () => {
	for (const file of ['build/lib/prerequisites.ts', 'build/lib/native.ts']) {
		const source = await readFile(join(root, file), 'utf8');
		for (const [, specifier] of source.matchAll(/^import (?!type )[^;]*from '([^']+)'/gm)) {
			assert(specifier.startsWith('node:') || specifier === './prerequisites.ts', `${file} imports ${specifier}`);
		}
	}
});

const mac = host({ platform: 'darwin', architecture: 'arm64', hardwareArchitecture: 'arm64', osName: 'macOS 27.0', distribution: undefined });

test('the development VniLink\'s screen recording grant is only settled by the prompt at the run', async () => {
	const vnilink = await checkRequirement('vnilink-screen-recording', manifest, mac, fakeProbe());
	assert.equal(vnilink.status, 'unknown');
	assert.equal(vnilink.kind, 'permission');
	assert.match(vnilink.fix[0], /allow VniLink in System Settings > Privacy & Security > Screen & System Audio Recording/i);
});

test('Linux, Windows and root permissions are checked where they can be', async () => {
	const [hostGrant, readerGrant] = latencyPortalGrants('/repo');
	assert.deepEqual([hostGrant, readerGrant], ['/repo/.build/latency/host-screen-grant', '/repo/.build/latency/reader-screen-grant']);
	const asked = await checkRequirement('screen-portal', manifest, host(), fakeProbe());
	assert.equal(asked.status, 'unknown');
	assert.match(asked.detail, /asks someone at this computer/);
	assert.match(asked.fix[0], /choose the display in both portal dialogs/);
	assert.equal((await checkRequirement('screen-portal', manifest, host(), fakeProbe({ files: [hostGrant] }))).status, 'unknown', 'both answers are needed');
	assert.equal((await checkRequirement('screen-portal', manifest, host(), fakeProbe({ files: [hostGrant, readerGrant] }))).status, 'ready');
	const windows = env => host({ platform: 'win32', distribution: undefined, env });
	const desktop = await checkRequirement('signed-in-desktop', manifest, windows({ SESSIONNAME: 'Console' }), fakeProbe());
	assert.equal(desktop.status, 'ready');
	assert.equal(desktop.detail, 'session Console');
	const ssh = await checkRequirement('signed-in-desktop', manifest, windows({}), fakeProbe());
	assert.equal(ssh.status, 'missing');
	assert.match(ssh.detail, /no signed-in desktop/);
	assert.deepEqual(ssh.fix, ['Run the task from a terminal on the signed-in desktop.']);
	assert.equal((await checkRequirement('root', manifest, host({ uid: 0 }), fakeProbe())).status, 'ready');
	const user = await checkRequirement('root', manifest, host({ uid: 1000 }), fakeProbe());
	assert.equal(user.status, 'missing');
	assert.match(user.fix[0], /npm run measure:impaired/);
	const [impaired] = await checkTasks(manifest, host({ uid: 1000 }), fakeProbe(), ['measure:impaired:linux']);
	assert.match(impaired.reason, /Root/);
});

/** A Mac with everything the build needs, so a task's own requirements decide its status. */
function toolsMac() {
	return fakeProbe({
		contents: repoFiles,
		files: ['/usr/bin/rustup', '/usr/bin/git', '/usr/bin/curl', '/usr/bin/cmake', '/repo/node_modules/@vnilink/elg/prebuilds/darwin-arm64/node.napi.node'],
		runs: { 'rustup toolchain list': '1.91.0-aarch64-apple-darwin\n', 'xcode-select -p': '/Library/Developer/CommandLineTools\n', 'xcrun --find clang': '/usr/bin/clang\n' },
	});
}

test('a task is ready only when it can run here; a second computer is optional and never needed', async () => {
	// The latency harness on macOS: ready but for the grant only its run can show.
	const [latency] = await checkTasks(manifest, mac, toolsMac(), ['measure:latency']);
	assert.equal(latency.status, 'unknown');
	assert.equal(latency.reason, 'cannot be checked before it runs: Screen Recording for the development VniLink');
	const other = latency.results.find(result => result.id === 'second-computer');
	assert.deepEqual(other, {
		id: 'second-computer', title: 'A second computer', kind: 'computer', status: 'unknown', optional: true,
		detail: '--peer runs a suite or a measurement\'s other side against `vnilink-testkit agent` there, built from the same commit, once a person types this computer\'s digits on its terminal; every task runs on this computer without it', fix: [], packages: [],
	});
	assert.equal(latency.results.find(result => result.id === 'ssh'), undefined, 'no task reaches another computer over SSH');
	const [build] = await checkTasks(manifest, mac, toolsMac(), ['build']);
	assert.equal(build.status, 'ready');
	assert.equal(build.reason, undefined);
	const text = formatDoctor([build, latency], 'header', [], []);
	assert.match(text, /\n\nOther computers\n {2}optional {4}A second computer: --peer runs a suite or a measurement's other side/);
	assert.match(text, /\nTasks\n {2}ready {7}build\n {2}unknown {5}measure:latency: cannot be checked before it runs: Screen Recording for the development VniLink$/m);
});

test('requirement and task entries have only known fields, and every task can run on one computer', () => {
	const requirementFields = ['title', 'kind', 'purpose', 'platforms', 'optional', 'prepared', 'check', 'fix'];
	const taskFields = ['description', 'run', 'platforms', 'requires', 'same'];
	for (const [id, requirement] of Object.entries(manifest.requirements)) {
		assert.deepEqual(Object.keys(requirement).filter(field => !requirementFields.includes(field)), [], id);
		assert.match(requirement.kind ?? 'tool', /^(tool|permission|computer)$/, id);
		if (requirement.kind === 'permission') {
			// A permission is the OS's: each names its OS and how to grant it.
			assert.ok(requirement.platforms?.length, `${id} names its OS`);
			assert.ok(requirement.fix?.default, `${id} says how to grant it`);
		}
		if (requirement.kind === 'computer') {
			assert.equal(requirement.optional, true, `${id}: a second computer is never required`);
		}
	}
	for (const [name, entry] of Object.entries(manifest.tasks)) {
		assert.deepEqual(Object.keys(entry).filter(field => !taskFields.includes(field)), [], name);
	}
	// What the runners call, by task.
	const needs = {
		'check:hygiene': ['git'],
		'check:license': ['git'],
		'license:apply': ['git'],
		'vscode': ['git'],
		'measure:impaired:linux': ['root', 'network-namespaces'],
		'measure:latency': ['vnilink-screen-recording', 'screen-portal', 'signed-in-desktop'],
	};
	for (const [task, ids] of Object.entries(needs)) {
		const declared = taskRequirements(manifest, task);
		assert.deepEqual(ids.filter(id => !declared.includes(id)), [], task);
		assert.ok(ids.every(id => !manifest.requirements[id].optional), `${task} cannot run without ${ids.join(', ')}`);
	}
	// --peer runs reach the test agent on a second computer, which is optional, and nothing uses SSH.
	assert.equal(manifest.requirements.ssh, undefined, 'no requirement for SSH');
	assert.equal(manifest.requirements['second-computer'].optional, true);
	for (const task of ['test:service', 'bench:service', 'measure:latency']) {
		assert.ok(taskRequirements(manifest, task).includes('second-computer'), `${task} lists the second computer`);
	}
});
