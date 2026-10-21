import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile, access } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
import { bundleOptions, relinkSourceFilter, releaseSourceFilter } from '../../build/lib/bundles.ts';
import { assertNativeFresh, compileNative, nativeEnvironment, nativeIdentity, stageGlass, stageNative } from '../../build/lib/native.ts';
import { loadProduct } from '../../build/lib/product.ts';
import { buildRelease } from '../../build/lib/release.ts';
import { verifyReleaseFiles } from '../../build/lib/verify.ts';

const repository = fileURLToPath(new URL('../../', import.meta.url));

async function put(root, name, content = 'fixture') {
	await mkdir(dirname(join(root, name)), { recursive: true });
	await writeFile(join(root, name), content);
}

test('release entry points exclude tests and compile with production configuration', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-release-bundles-'));
	try {
		const outputDirectory = join(root, 'release-app');
		const options = bundleOptions(root, { production: true, outputDirectory });
		assert.equal(options.length, 6);
		assert.equal(bundleOptions(root).length, 8);
		await put(root, 'tsconfig.json', '{}');
		for (const option of options) {
			await put(root, option.entryPoints[0], 'console.log(process.env.NODE_ENV);');
			await build(option);
			assert.match(await readFile(option.outfile, 'utf8'), /production/);
			assert(!option.entryPoints[0].startsWith('test/'));
		}
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('release native profile is passed to Cargo and isolated from dev staging', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-release-native-'));
	try {
		for (const directory of ['crates', 'vendor', '.cargo', 'target/electron/release', 'release/native']) {
			await mkdir(join(root, directory), { recursive: true });
		}
		for (const name of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']) {
			await put(root, name);
		}
		await put(root, 'out/native/vnilink.node', 'dev-sentinel');
		let invoked;
		const library = process.platform === 'win32' ? 'vnilink_node.dll' : process.platform === 'darwin' ? 'libvnilink_node.dylib' : 'libvnilink_node.so';
		const options = { profile: 'release' };
		const built = await compileNative(root, {
			run: async (command, args, runOptions) => {
				invoked = { command, args, options: runOptions };
				await put(root, `target/electron/release/${library}`, 'release-binary');
			}
		}, options);
		await stageNative(root, join(root, 'release'), options, built);
		await assertNativeFresh(root, join(root, 'release'), options);
		assert(invoked.args.includes('--release'));
		assert.equal(await readFile(join(root, 'out/native/vnilink.node'), 'utf8'), 'dev-sentinel');
		await put(root, 'release/native/vnilink.node', 'tampered');
		await assert.rejects(assertNativeFresh(root, join(root, 'release'), options), /stale/);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('Liquid Glass is staged from the optional package on macOS only', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-glass-'));
	try {
		const output = join(root, 'out');
		await mkdir(join(output, 'licenses'), { recursive: true });
		assert.equal(await stageGlass(root, output, 'win32', 'x64'), false);
		assert.equal(await stageGlass(root, output, 'linux', 'x64'), false);
		await assert.rejects(stageGlass(root, output, 'darwin', 'arm64'), /Liquid Glass is missing for darwin-arm64: run npm ci/);
		const pkg = join(root, 'node_modules/@vnilink/elg');
		await put(pkg, 'dist/index.cjs', 'owner');
		await put(pkg, 'prebuilds/darwin-arm64/node.napi.node', 'arm64-addon');
		await put(pkg, 'prebuilds/darwin-x64/node.napi.node', 'x64-addon');
		await put(pkg, 'LICENSE', 'MIT');
		assert.equal(await stageGlass(root, output, 'darwin', 'arm64'), true);
		assert.equal(await readFile(join(output, 'native/elg/index.cjs'), 'utf8'), 'owner');
		assert.equal(await readFile(join(output, 'native/elg/elg.node'), 'utf8'), 'arm64-addon', 'Staged the wrong architecture');
		assert.equal(await readFile(join(output, 'licenses/elg.txt'), 'utf8'), 'MIT');
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('the GNU Windows target finds its MinGW tools without shell setup', () => {
	const options = { profile: 'dev', target: 'x86_64-pc-windows-gnu' };
	const files = (...paths) => path => paths.includes(path);

	// A runner or custom install already on PATH is used as it is, wherever it lives.
	const runner = 'D:\\a\\_temp\\msys64\\mingw64\\bin';
	const onPath = nativeEnvironment(options, { Path: `C:\\Windows;${runner}` }, files(`${runner}\\dlltool.exe`, `${runner}\\libclang.dll`));
	assert.equal(onPath.Path, `C:\\Windows;${runner}`, 'A toolchain on PATH was replaced or moved');
	assert.equal(onPath.LIBCLANG_PATH, runner);
	assert.equal(onPath.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER, 'gcc');
	assert.equal(onPath.CMAKE_GENERATOR, 'MinGW Makefiles');

	// A plain MSYS2 install needs no shell setup.
	const bin = 'C:\\msys64\\mingw64\\bin';
	const plain = nativeEnvironment(options, { Path: 'C:\\Windows;C:\\Users\\me\\.cargo\\bin' }, files(`${bin}\\dlltool.exe`));
	assert.equal(plain.Path, `${bin};C:\\Windows;C:\\Users\\me\\.cargo\\bin`);
	assert.equal(plain.PATH, undefined, 'A second PATH key leaves the winner to the spawn');
	assert.equal(plain.LIBCLANG_PATH, undefined, 'LIBCLANG_PATH named a folder without libclang');

	// Another root is named explicitly, and caller choices are kept.
	const custom = 'D:\\tools\\msys64\\mingw64\\bin';
	const named = nativeEnvironment(options, { VNILINK_MSYS_ROOT: 'D:\\tools\\msys64', CMAKE_GENERATOR: 'Ninja' }, files(`${custom}\\dlltool.exe`, `C:\\msys64\\mingw64\\bin\\dlltool.exe`));
	assert.equal(named.PATH, custom, 'VNILINK_MSYS_ROOT lost to the default root');
	assert.equal(named.CMAKE_GENERATOR, 'Ninja', 'A caller choice was overridden');
	assert.throws(() => nativeEnvironment(options, { Path: 'C:\\Windows' }, () => false), /found none on PATH or in C:\\msys64\\mingw64\\bin.*VNILINK_MSYS_ROOT/);
	assert.deepEqual(nativeEnvironment({ profile: 'dev', target: 'x86_64-pc-windows-msvc' }, { Path: 'C:\\Windows' }, () => false), { Path: 'C:\\Windows' });
});

test('a macOS minimum reaches only the target triple, never host build tools', async () => {
	const options = { profile: 'release', target: 'aarch64-apple-darwin', minimumSystemVersion: '14.0' };
	const environment = nativeEnvironment(options, { PATH: '/bin', MACOSX_DEPLOYMENT_TARGET: '11.0', CFLAGS_aarch64_apple_darwin: '-O2' });
	assert.equal(environment.MACOSX_DEPLOYMENT_TARGET, undefined, 'A global deployment target would reach host proc macros');
	assert.equal(environment.CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS, '-C link-arg=-mmacosx-version-min=14.0');
	assert.equal(environment.CFLAGS_aarch64_apple_darwin, '-O2 -mmacosx-version-min=14.0');
	assert.equal(environment.CXXFLAGS_aarch64_apple_darwin, '-mmacosx-version-min=14.0');
	assert.equal(environment.PATH, '/bin');
	assert.throws(() => nativeEnvironment({ profile: 'release', minimumSystemVersion: '14.0' }, {}), /explicit Apple target/);
	assert.throws(() => nativeEnvironment({ ...options, target: 'x86_64-unknown-linux-gnu' }, {}), /explicit Apple target/);
	assert.throws(() => nativeEnvironment(options, { RUSTFLAGS: '-C opt-level=1' }), /RUSTFLAGS/);
	assert.deepEqual(nativeEnvironment({ profile: 'release', target: 'x86_64-unknown-linux-gnu' }, { PATH: '/bin' }), { PATH: '/bin' });
	const root = await mkdtemp(join(tmpdir(), 'vnilink-native-minimum-'));
	try {
		for (const name of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']) {
			await put(root, name);
		}
		for (const directory of ['crates', 'vendor', '.cargo']) {
			await mkdir(join(root, directory), { recursive: true });
		}
		const identity = await nativeIdentity(root, options);
		assert.equal(identity.minimumSystemVersion, '14.0');
		assert.notDeepEqual(identity, await nativeIdentity(root, { ...options, minimumSystemVersion: '15.0' }), 'Builds for different minimums share an identity');
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('the addon identity follows crate sources, not a fuzz crate or its corpus', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-native-sources-'));
	try {
		for (const name of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'crates/core/src/lib.rs']) {
			await put(root, name);
		}
		for (const directory of ['vendor', '.cargo']) {
			await mkdir(join(root, directory), { recursive: true });
		}
		const options = { profile: 'dev' };
		const identity = await nativeIdentity(root, options);
		await put(root, 'crates/core/fuzz/corpus/target/input', 'new input');
		await put(root, 'crates/core/fuzz/fuzz_targets/target.rs', 'fn main() {}');
		assert.deepEqual(await nativeIdentity(root, options), identity, 'A fuzz crate changed the addon identity');
		await put(root, 'crates/core/src/lib.rs', 'changed');
		assert.notDeepEqual(await nativeIdentity(root, options), identity, 'A crate source left the addon identity unchanged');
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

const product = await loadProduct(repository);

const hostTarget = product.platforms[process.platform]?.targets.some(target => target.architecture === process.arch);

test('release publication inventories only complete payloads, cleans failures and preserves the previous release', { skip: !hostTarget && `product.json approves no ${process.platform}-${process.arch} release target` }, async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-release-stage-'));
	let fail = false;
	let mutate = false;
	try {
		await put(root, 'package.json', JSON.stringify({ version: '0.1.0', devDependencies: { electron: '44.7.0' } }));
		for (const directory of ['src', 'build', 'scripts', 'assets']) {
			await mkdir(join(root, directory), { recursive: true });
		}
		for (const path of ['package-lock.json', 'tsconfig.json', 'src/tsconfig.json', 'LICENSE.vscode.txt', 'ThirdPartyNotices.vscode.txt']) {
			await put(root, path);
		}
		await put(root, 'product.json', JSON.stringify(product));
		for (const platform of Object.values(product.platforms)) {
			await put(root, platform.icon.endsWith('.icon') ? `${platform.icon}/icon.json` : platform.icon, '{}');
		}
		await put(root, product.platforms.darwin.fallbackIconSource, '{}');
		await put(root, 'out/dev-sentinel');
		const fixture = {
			assertNativeFresh: async () => undefined,
			buildApplication: async (_root, _signal, options) => {
				assert.equal(options.production, true);
				assert.equal(options.native.profile, 'release');
				const glass = process.platform === 'darwin' ? ['native/elg/index.cjs', 'native/elg/elg.node', 'licenses/elg.txt'] : [];
				const names = ['main.cjs', 'engine.cjs', 'preload.cjs', 'renderer/renderer.js', 'renderer/renderer.css', 'renderer/index.html', 'noticePreload.cjs', 'renderer/notice.js', 'renderer/notice.css', 'renderer/notice.html', 'assets/tray/idleTemplate.png', 'native/vnilink.node', 'native/build.json', 'licenses/native-codecs/native-codecs.txt', 'relink/source/Cargo.lock', ...glass];
				for (const name of names) {
					await put(options.outputDirectory, name);
				}
				if (mutate) {
					await put(root, 'src/changed.ts');
				}
				if (fail) {
					await put(options.outputDirectory, 'smoke.cjs');
				}
			},
		};
		const release = () => buildRelease(root, new AbortController().signal, fixture);
		const output = await release();
		await verifyReleaseFiles(output);
		const manifest = JSON.parse(await readFile(join(output, 'build-manifest.json'), 'utf8'));
		assert.equal(manifest.profile, 'release');
		assert.equal(JSON.parse(await readFile(join(output, 'app/package.json'), 'utf8')).main, 'main.cjs');
		for (const file of manifest.files) {
			assert.equal(createHash('sha256').update(await readFile(join(output, file.path))).digest('hex'), file.sha256);
		}
		const previous = await readFile(join(output, 'build-manifest.json'), 'utf8');
		fail = true;
		await assert.rejects(release(), /Test bundles/);
		assert.equal(await readFile(join(output, 'build-manifest.json'), 'utf8'), previous);
		assert.equal(await readFile(join(root, 'out/dev-sentinel'), 'utf8'), 'fixture');
		assert.deepEqual(await readdir(join(root, '.build/release')), [`${process.platform}-${process.arch}`]);
		fail = false;
		mutate = true;
		await assert.rejects(release(), /inputs changed/);
		assert.equal(await readFile(join(output, 'build-manifest.json'), 'utf8'), previous);
		await access(join(output, 'app/assets/tray/idleTemplate.png'));
		await put(output, 'app/main.cjs', 'modified');
		await assert.rejects(verifyReleaseFiles(output), /inventory differs/);
		await mkdir(`${output}.lock`);
		await put(`${output}.lock`, 'owner', 'another-build');
		await assert.rejects(release(), /EEXIST/);
		assert.equal(await readFile(join(`${output}.lock`, 'owner'), 'utf8'), 'another-build');
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('relink staging excludes generated objects and tooling metadata while retaining source/license files', () => {
	for (const file of ['target', '.git', '.build', 'node_modules', 'AGENTS.md', 'libcodec.a', 'codec.o', 'addon.node', 'crates/core/fuzz']) {
		assert.equal(releaseSourceFilter(file), false, file);
	}
	assert.equal(relinkSourceFilter('crates/core/fuzz'), false, 'Development relink staging copies a fuzz crate');
	assert.equal(relinkSourceFilter('crates/core/src'), true);
	for (const file of ['build.rs', 'Cargo.lock', 'LICENSE', 'COPYING', 'VNILINK.md', 'source.cpp']) {
		assert.equal(releaseSourceFilter(file), true, file);
	}
});
