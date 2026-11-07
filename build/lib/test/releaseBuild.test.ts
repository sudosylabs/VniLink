// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { build } from 'esbuild';
import { createHash } from 'node:crypto';
import { access, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { bundleOptions, relinkSourceFilter, releaseSourceFilter, type ApplicationBuildOptions } from '../bundles.ts';
import { installerResources } from '../installers.ts';
import { acquireLock } from '../lock.ts';
import { assertNativeFresh, compileNative, nativeEnvironment, nativeIdentity, stageGlass, stageNative, stageSpdlog, type NativeOptions } from '../native.ts';
import { isMacintosh, isWindows } from '../platform.ts';
import { loadProduct, type ProductTarget } from '../product.ts';
import { buildRelease } from '../release.ts';
import { verifyReleaseFiles, type ReleaseManifest } from '../inventory.ts';

const product = await loadProduct(fileURLToPath(new URL('../../../', import.meta.url)));

async function put(root: string, name: string, content = 'fixture'): Promise<void> {
	await mkdir(dirname(join(root, name)), { recursive: true });
	await writeFile(join(root, name), content);
}

/** The inputs every addon identity reads: the workspace manifests, `crates/`, `vendor/` and `.cargo/`. */
async function nativeSources(root: string): Promise<void> {
	for (const name of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']) {
		await put(root, name);
	}
	for (const directory of ['crates', 'vendor', '.cargo']) {
		await mkdir(join(root, directory), { recursive: true });
	}
}

suite('release build', () => {
	let root: string;
	beforeEach(async () => {
		root = await mkdtemp(join(tmpdir(), 'vnilink-release-'));
	});
	afterEach(async () => {
		await rm(root, { recursive: true, force: true });
	});
	suite('bundles', () => {
		for (const [index, option] of bundleOptions('/root', { production: true }).entries()) {
			const [entry] = Array.isArray(option.entryPoints) ? option.entryPoints.filter(point => typeof point === 'string') : [''];
			test(`keeps tests out of release entry point ${entry}`, () => {
				assert.ok(entry.length > 0 && !entry.startsWith('test/'));
			});
			test(`compiles ${entry} with the production configuration`, async () => {
				const release = bundleOptions(root, { production: true, outputDirectory: join(root, 'release-app') })[index];
				await put(root, 'tsconfig.json', '{}');
				await put(root, entry, 'console.log(process.env.NODE_ENV);');
				await build(release);
				assert.match(await readFile(release.outfile ?? '', 'utf8'), /production/);
			});
		}
	});
	suite('native addon', () => {
		const options: NativeOptions = { profile: 'release' };
		const library = isWindows ? 'vnilink_node.dll' : isMacintosh ? 'libvnilink_node.dylib' : 'libvnilink_node.so';
		let cargo: readonly string[];
		beforeEach(async () => {
			await nativeSources(root);
			for (const directory of ['target/electron/release', 'release/native']) {
				await mkdir(join(root, directory), { recursive: true });
			}
			await put(root, 'out/native/vnilink.node', 'dev-sentinel');
			const built = await compileNative(root, {
				run: async (_command, args) => {
					cargo = args;
					await put(root, `target/electron/release/${library}`, 'release-binary');
					return 0;
				}
			}, options);
			await stageNative(root, join(root, 'release'), options, built);
		});
		test('passes the release profile to Cargo', () => {
			assert.ok(cargo.includes('--release'));
		});
		test('leaves the development addon alone', async () => {
			assert.strictEqual(await readFile(join(root, 'out/native/vnilink.node'), 'utf8'), 'dev-sentinel');
		});
		test('accepts the release addon it staged', async () => {
			await assertNativeFresh(root, join(root, 'release'), options);
		});
		test('refuses a staged addon changed since', async () => {
			await put(root, 'release/native/vnilink.node', 'tampered');
			await assert.rejects(assertNativeFresh(root, join(root, 'release'), options), /stale/);
		});
	});
	suite('Liquid Glass', () => {
		let output: string;
		beforeEach(async () => {
			output = join(root, 'out');
			await mkdir(join(output, 'licenses'), { recursive: true });
		});
		async function installGlass(): Promise<void> {
			const pkg = join(root, 'node_modules/@vnilink/elg');
			await put(pkg, 'dist/index.cjs', 'owner');
			await put(pkg, 'prebuilds/darwin-arm64/node.napi.node', 'arm64-addon');
			await put(pkg, 'prebuilds/darwin-x64/node.napi.node', 'x64-addon');
			await put(pkg, 'LICENSE', 'MIT');
		}
		for (const platform of ['win32', 'linux'] as const) {
			test(`stages nothing on ${platform}`, async () => {
				assert.strictEqual(await stageGlass(root, output, platform, 'x64'), false);
			});
		}
		test('refuses a macOS build without the package', async () => {
			await assert.rejects(stageGlass(root, output, 'darwin', 'arm64'), /Liquid Glass is missing for darwin-arm64: run npm ci/);
		});
		test('stages the package on macOS', async () => {
			await installGlass();
			assert.strictEqual(await stageGlass(root, output, 'darwin', 'arm64'), true);
		});
		for (const [file, content] of [['native/elg/index.cjs', 'owner'], ['native/elg/elg.node', 'arm64-addon'], ['licenses/elg.txt', 'MIT']] as const) {
			test(`stages ${file} for the architecture built`, async () => {
				await installGlass();
				await stageGlass(root, output, 'darwin', 'arm64');
				assert.strictEqual(await readFile(join(output, file), 'utf8'), content);
			});
		}
	});
	suite('spdlog', () => {
		async function installSpdlog(): Promise<void> {
			const pkg = join(root, 'node_modules/@vscode/spdlog');
			await put(pkg, 'build/Release/spdlog.node', 'addon');
			await put(pkg, 'LICENSE', 'MIT Microsoft');
			await put(pkg, 'deps/spdlog/LICENSE', 'MIT Gabi');
		}
		test('skips spdlog when the optional package did not build', async () => {
			assert.strictEqual(await stageSpdlog(root, join(root, 'out')), false);
		});
		test('stages spdlog when the optional package built', async () => {
			await installSpdlog();
			assert.strictEqual(await stageSpdlog(root, join(root, 'out')), true);
		});
		for (const [file, content] of [['native/spdlog/spdlog.node', 'addon'], ['licenses/vscode-spdlog.txt', 'MIT Microsoft'], ['licenses/spdlog.txt', 'MIT Gabi']] as const) {
			test(`stages ${file} with spdlog`, async () => {
				await installSpdlog();
				await stageSpdlog(root, join(root, 'out'));
				assert.strictEqual(await readFile(join(root, 'out', file), 'utf8'), content);
			});
		}
	});
	suite('Windows GNU environment', () => {
		const options: NativeOptions = { profile: 'dev', target: 'x86_64-pc-windows-gnu' };
		const files = (...paths: string[]) => (path: string) => paths.includes(path);

		// A runner or custom install already on PATH is used as it is, wherever it lives.
		const runner = 'D:\\a\\_temp\\msys64\\mingw64\\bin';
		const onPath = nativeEnvironment(options, { Path: `C:\\Windows;${runner}` }, files(`${runner}\\dlltool.exe`, `${runner}\\libclang.dll`));
		test('keeps a toolchain already on PATH where it is', () => {
			assert.strictEqual(onPath.Path, `C:\\Windows;${runner}`);
		});
		test('finds libclang beside a toolchain on PATH', () => {
			assert.strictEqual(onPath.LIBCLANG_PATH, runner);
		});
		test('links with gcc', () => {
			assert.strictEqual(onPath.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER, 'gcc');
		});
		test('generates MinGW Makefiles', () => {
			assert.strictEqual(onPath.CMAKE_GENERATOR, 'MinGW Makefiles');
		});

		// A plain MSYS2 install needs no shell setup.
		const bin = 'C:\\msys64\\mingw64\\bin';
		const plain = nativeEnvironment(options, { Path: 'C:\\Windows;C:\\Users\\someone\\.cargo\\bin' }, files(`${bin}\\dlltool.exe`));
		test('puts a plain MSYS2 install first on PATH', () => {
			assert.strictEqual(plain.Path, `${bin};C:\\Windows;C:\\Users\\someone\\.cargo\\bin`);
		});
		test('keeps one PATH key, so the spawn has no winner to pick', () => {
			assert.strictEqual(plain.PATH, undefined);
		});
		test('leaves LIBCLANG_PATH unset beside a toolchain without libclang', () => {
			assert.strictEqual(plain.LIBCLANG_PATH, undefined);
		});

		// Another root is named explicitly, and caller choices are kept.
		const custom = 'D:\\tools\\msys64\\mingw64\\bin';
		const named = nativeEnvironment(options, { VNILINK_MSYS_ROOT: 'D:\\tools\\msys64', CMAKE_GENERATOR: 'Ninja' }, files(`${custom}\\dlltool.exe`, `C:\\msys64\\mingw64\\bin\\dlltool.exe`));
		test('prefers VNILINK_MSYS_ROOT to the default root', () => {
			assert.strictEqual(named.PATH, custom);
		});
		test('keeps the caller\'s CMake generator', () => {
			assert.strictEqual(named.CMAKE_GENERATOR, 'Ninja');
		});
		test('says where it looked when it finds no toolchain', () => {
			assert.throws(() => nativeEnvironment(options, { Path: 'C:\\Windows' }, () => false), /found none on PATH or in C:\\msys64\\mingw64\\bin.*VNILINK_MSYS_ROOT/);
		});
		test('leaves the MSVC target\'s environment as it is', () => {
			assert.deepStrictEqual(nativeEnvironment({ profile: 'dev', target: 'x86_64-pc-windows-msvc' }, { Path: 'C:\\Windows' }, () => false), { Path: 'C:\\Windows' });
		});
	});
	suite('macOS minimum', () => {
		const options: NativeOptions = { profile: 'release', target: 'aarch64-apple-darwin', minimumSystemVersion: '14.0' };
		const environment = nativeEnvironment(options, { PATH: '/bin', MACOSX_DEPLOYMENT_TARGET: '11.0', CFLAGS_aarch64_apple_darwin: '-O2' });
		test('unsets a global deployment target, which would reach host proc macros', () => {
			assert.strictEqual(environment.MACOSX_DEPLOYMENT_TARGET, undefined);
		});
		test('gives the minimum to the target\'s linker', () => {
			assert.strictEqual(environment.CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS, '-C link-arg=-mmacosx-version-min=14.0');
		});
		test('appends the minimum to the target\'s C flags', () => {
			assert.strictEqual(environment.CFLAGS_aarch64_apple_darwin, '-O2 -mmacosx-version-min=14.0');
		});
		test('gives the minimum to the target\'s C++ flags', () => {
			assert.strictEqual(environment.CXXFLAGS_aarch64_apple_darwin, '-mmacosx-version-min=14.0');
		});
		test('keeps the rest of the environment', () => {
			assert.strictEqual(environment.PATH, '/bin');
		});
		for (const [description, target] of [['without a target', undefined], ['for a Linux target', 'x86_64-unknown-linux-gnu']] as const) {
			test(`refuses a minimum ${description}`, () => {
				assert.throws(() => nativeEnvironment({ ...options, target }, {}), /explicit Apple target/);
			});
		}
		test('refuses RUSTFLAGS, which would replace the minimum', () => {
			assert.throws(() => nativeEnvironment(options, { RUSTFLAGS: '-C opt-level=1' }), /RUSTFLAGS/);
		});
		test('leaves a build without a minimum as it is', () => {
			assert.deepStrictEqual(nativeEnvironment({ profile: 'release', target: 'x86_64-unknown-linux-gnu' }, { PATH: '/bin' }), { PATH: '/bin' });
		});
		test('records the minimum in the addon identity', async () => {
			await nativeSources(root);
			assert.strictEqual((await nativeIdentity(root, options)).minimumSystemVersion, '14.0');
		});
		test('gives builds for different minimums different identities', async () => {
			await nativeSources(root);
			assert.notDeepStrictEqual(await nativeIdentity(root, options), await nativeIdentity(root, { ...options, minimumSystemVersion: '15.0' }));
		});
	});
	suite('addon identity', () => {
		const options: NativeOptions = { profile: 'dev' };
		beforeEach(async () => {
			await nativeSources(root);
			await put(root, 'crates/core/src/lib.rs');
		});
		test('ignores a fuzz crate and its corpus', async () => {
			const identity = await nativeIdentity(root, options);
			await put(root, 'crates/core/fuzz/corpus/target/input', 'new input');
			await put(root, 'crates/core/fuzz/fuzz_targets/target.rs', 'fn main() {}');
			assert.deepStrictEqual(await nativeIdentity(root, options), identity);
		});
		test('follows a crate source', async () => {
			const identity = await nativeIdentity(root, options);
			await put(root, 'crates/core/src/lib.rs', 'changed');
			assert.notDeepStrictEqual(await nativeIdentity(root, options), identity);
		});
	});
	const platforms: Readonly<Record<string, { readonly targets: readonly ProductTarget[] } | undefined>> = product.platforms;
	const hostTarget = platforms[process.platform]?.targets.some(target => target.architecture === process.arch) === true;
	suite('publication', { skip: !hostTarget && `product.json approves no ${process.platform}-${process.arch} release target` }, () => {
		let builds: ApplicationBuildOptions[];
		let failing: boolean;
		let changing: boolean;
		const release = () => buildRelease(root, new AbortController().signal, {
			assertNativeFresh: () => Promise.resolve(),
			buildApplication: async (_root, _signal, options) => {
				builds.push(options);
				const output = options.outputDirectory ?? '';
				const glass = isMacintosh ? ['native/elg/index.cjs', 'native/elg/elg.node', 'licenses/elg.txt'] : [];
				const names = [
					'main.cjs', 'engine.cjs', 'preload.cjs', 'renderer/renderer.js', 'renderer/renderer.css', 'renderer/index.html', 'noticePreload.cjs', 'renderer/notice.js',
					'renderer/notice.css', 'renderer/notice.html', 'assets/tray/idleTemplate.png', 'native/vnilink.node', 'native/build.json',
					'licenses/native-codecs/native-codecs.txt', 'relink/source/Cargo.lock', ...glass
				];
				for (const name of names) {
					await put(output, name);
				}
				if (changing) {
					await put(root, 'src/changed.ts');
				}
				if (failing) {
					await put(output, 'smoke.cjs');
				}
			},
		});
		const manifest = async (output: string) => JSON.parse(await readFile(join(output, 'build-manifest.json'), 'utf8')) as ReleaseManifest;
		beforeEach(async () => {
			builds = [];
			failing = false;
			changing = false;
			await put(root, 'package.json', JSON.stringify({ version: '0.1.0', devDependencies: { electron: '44.7.0' } }));
			for (const directory of ['src', 'build', 'scripts', 'assets']) {
				await mkdir(join(root, directory), { recursive: true });
			}
			for (const path of ['package-lock.json', 'tsconfig.json', 'LICENSE', 'LICENSE.vscode.txt', 'ThirdPartyNotices.vscode.txt']) {
				await put(root, path);
			}
			await put(root, 'product.json', JSON.stringify(product));
			for (const platform of Object.values(product.platforms)) {
				await put(root, platform.icon.endsWith('.icon') ? `${platform.icon}/icon.json` : platform.icon, '{}');
			}
			await put(root, product.platforms.darwin.fallbackIconSource, '{}');
			for (const resource of installerResources(process.platform)) {
				await put(root, resource);
			}
			await put(root, 'out/dev-sentinel');
		});
		test('builds the application for production', async () => {
			await release();
			assert.deepStrictEqual(builds.map(options => options.production), [true]);
		});
		test('builds the addon with the release profile', async () => {
			await release();
			assert.deepStrictEqual(builds.map(options => options.native?.profile), ['release']);
		});
		test('publishes an inventory that verifies', async () => {
			await verifyReleaseFiles(await release());
		});
		test('records the release profile', async () => {
			assert.strictEqual((await manifest(await release())).profile, 'release');
		});
		test('starts the application at main.cjs', async () => {
			const output = await release();
			assert.strictEqual((JSON.parse(await readFile(join(output, 'app/package.json'), 'utf8')) as { readonly main: string }).main, 'main.cjs');
		});
		test('records the digest of every file it publishes', async () => {
			const output = await release();
			for (const file of (await manifest(output)).files) {
				assert.strictEqual(createHash('sha256').update(await readFile(join(output, file.path))).digest('hex'), file.sha256, file.path);
			}
		});
		test('stages the resources the host\'s installer reads beside its icon', async () => {
			const stage = await release();
			for (const resource of installerResources(process.platform)) {
				await access(join(stage, 'packaging-resources', resource));
			}
		});
		test('publishes the tray assets', async () => {
			await access(join(await release(), 'app/assets/tray/idleTemplate.png'));
		});
		test('refuses a payload with test bundles', async () => {
			failing = true;
			await assert.rejects(release(), /Test bundles/);
		});
		test('keeps the previous release when a build fails', async () => {
			const output = await release();
			const previous = await readFile(join(output, 'build-manifest.json'), 'utf8');
			failing = true;
			await assert.rejects(release());
			assert.strictEqual(await readFile(join(output, 'build-manifest.json'), 'utf8'), previous);
		});
		test('leaves the development output alone', async () => {
			failing = true;
			await assert.rejects(release());
			assert.strictEqual(await readFile(join(root, 'out/dev-sentinel'), 'utf8'), 'fixture');
		});
		test('cleans a failed build\'s staging', async () => {
			await release();
			failing = true;
			await assert.rejects(release());
			assert.deepStrictEqual(await readdir(join(root, '.build/release')), [`${process.platform}-${process.arch}`]);
		});
		test('refuses a release whose inputs changed during the build', async () => {
			changing = true;
			await assert.rejects(release(), /inputs changed/);
		});
		test('keeps the previous release when inputs changed during the build', async () => {
			const output = await release();
			const previous = await readFile(join(output, 'build-manifest.json'), 'utf8');
			changing = true;
			await assert.rejects(release());
			assert.strictEqual(await readFile(join(output, 'build-manifest.json'), 'utf8'), previous);
		});
		test('refuses a published file that differs from its inventory', async () => {
			const output = await release();
			await put(output, 'app/main.cjs', 'modified');
			await assert.rejects(verifyReleaseFiles(output), /inventory differs/);
		});
		// A build still running holds the lock: this process stands in for it.
		async function holdLock(): Promise<string> {
			const output = await release();
			await mkdir(`${output}.lock`);
			await put(`${output}.lock`, 'owner.json', JSON.stringify({ pid: process.pid }));
			return output;
		}
		test('refuses to publish while another build holds the lock', async () => {
			await holdLock();
			await assert.rejects(release(), /is publishing here/);
		});
		test('leaves another build\'s lock in place', async () => {
			const output = await holdLock();
			await assert.rejects(release());
			assert.strictEqual(await readFile(join(`${output}.lock`, 'owner.json'), 'utf8'), JSON.stringify({ pid: process.pid }));
		});
	});
	suite('relink sources', () => {
		for (const file of ['target', '.git', '.build', 'node_modules', 'AGENTS.md', 'libcodec.a', 'codec.o', 'addon.node', 'crates/core/fuzz']) {
			test(`leaves ${file} out of release sources`, () => {
				assert.strictEqual(releaseSourceFilter(file), false);
			});
		}
		for (const file of ['build.rs', 'Cargo.lock', 'LICENSE', 'COPYING', 'VNILINK.md', 'source.cpp']) {
			test(`keeps ${file} in release sources`, () => {
				assert.strictEqual(releaseSourceFilter(file), true);
			});
		}
		test('leaves a fuzz crate out of development relink sources', () => {
			assert.strictEqual(relinkSourceFilter('crates/core/fuzz'), false);
		});
		test('keeps crate sources in development relink sources', () => {
			assert.strictEqual(relinkSourceFilter('crates/core/src'), true);
		});
	});
	suite('publish lock', () => {
		let lock: string;
		beforeEach(() => {
			lock = join(root, 'out.lock');
		});
		test('refuses a lock this running process holds, naming it', async () => {
			const release = await acquireLock(lock);
			await assert.rejects(acquireLock(lock), new RegExp(`process ${String(process.pid)}\\) is publishing here`));
			await release();
		});
		test('is free again once released', async () => {
			await (await acquireLock(lock))();
			await (await acquireLock(lock))();
		});
		test('takes over a lock a stopped build left behind', async () => {
			await mkdir(lock);
			await writeFile(join(lock, 'owner.json'), JSON.stringify({ pid: 2 ** 22 + 12345 }));
			await (await acquireLock(lock))();
		});
		test('takes over a lock that names no owner', async () => {
			await mkdir(lock);
			await (await acquireLock(lock))();
		});
	});
});
