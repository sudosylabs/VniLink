import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile, access } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
import { bundleOptions, releaseSourceFilter } from '../../build/lib/bundles.ts';
import { assertNativeFresh, compileNative, stageNative } from '../../build/lib/native.ts';
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
		assert.equal(options.length, 4);
		assert.equal(bundleOptions(root).length, 6);
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

test('release native profile and deployment baseline are passed to Cargo and isolated from dev staging', async () => {
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
		const options = { profile: 'release', environment: { MACOSX_DEPLOYMENT_TARGET: '14.0' } };
		const built = await compileNative(root, {
			run: async (command, args, runOptions) => {
				invoked = { command, args, options: runOptions };
				await put(root, `target/electron/release/${library}`, 'release-binary');
			}
		}, options);
		await stageNative(root, join(root, 'release'), options, built);
		await assertNativeFresh(root, join(root, 'release'), options);
		assert(invoked.args.includes('--release'));
		assert.equal(invoked.options.env.MACOSX_DEPLOYMENT_TARGET, '14.0');
		assert.equal(await readFile(join(root, 'out/native/vnilink.node'), 'utf8'), 'dev-sentinel');
		await put(root, 'release/native/vnilink.node', 'tampered');
		await assert.rejects(assertNativeFresh(root, join(root, 'release'), options), /stale/);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('release publication inventories only complete payloads, cleans failures and preserves the previous release', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-release-stage-'));
	const product = await loadProduct(repository);
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
		await put(root, 'assets/tray/trayTemplate.png');
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
				const names = ['main.cjs', 'engine.cjs', 'preload.cjs', 'renderer/renderer.js', 'renderer/renderer.css', 'renderer/index.html', 'native/vnilink.node', 'native/build.json', 'licenses/native-codecs/native-codecs.txt', 'relink/source/Cargo.lock'];
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
		await access(join(output, 'app/assets/tray/trayTemplate.png'));
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
	for (const file of ['target', '.git', '.build', 'node_modules', 'AGENTS.md', 'libcodec.a', 'codec.o', 'addon.node']) {
		assert.equal(releaseSourceFilter(file), false, file);
	}
	for (const file of ['build.rs', 'Cargo.lock', 'LICENSE', 'COPYING', 'VNILINK.md', 'source.cpp']) {
		assert.equal(releaseSourceFilter(file), true, file);
	}
});
