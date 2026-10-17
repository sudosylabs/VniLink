import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile, access } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { build } from 'esbuild';
import { buildOptions } from '../../build/tasks.mjs';
import { ReleaseVerifier } from '../../build/lib/releaseVerifier.mjs';
import { ReleaseBuild } from '../../build/lib/releaseBuild.mjs';
import { NativeBuild } from '../../build/lib/nativeBuild.mjs';
import { releaseSourceFilter } from '../../build/lib/releaseFiles.mjs';
import { loadProduct } from '../../build/product.mjs';

async function put(root, name, content = 'fixture') {
	await mkdir(dirname(join(root, name)), { recursive: true });
	await writeFile(join(root, name), content);
}

test('release entry points exclude tests and compile with production configuration', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-release-bundles-'));
	try {
		const outputDirectory = join(root, 'release-app');
		const options = buildOptions(root, { production: true, outputDirectory });
		assert.equal(options.length, 4);
		assert.equal(buildOptions(root).length, 6);
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
		const native = new NativeBuild(root, {
			run: async (command, args, options) => {
				invoked = { command, args, options };
				await put(root, `target/electron/release/${library}`, 'release-binary');
			}
		}, null, 'release', { MACOSX_DEPLOYMENT_TARGET: '14.0' });
		await native.build();
		await native.stage(join(root, 'release'));
		await native.assertFresh(join(root, 'release'));
		assert(invoked.args.includes('--release'));
		assert.equal(invoked.options.env.MACOSX_DEPLOYMENT_TARGET, '14.0');
		assert.equal(await readFile(join(root, 'out/native/vnilink.node'), 'utf8'), 'dev-sentinel');
		await put(root, 'release/native/vnilink.node', 'tampered');
		await assert.rejects(native.assertFresh(join(root, 'release')), /stale/);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('release publication inventories only complete payloads, cleans failures and preserves the previous release', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-release-stage-'));
	const product = await loadProduct();
	let stopped = 0;
	let fail = false;
	let mutate = false;
	try {
		await put(root, 'package.json', JSON.stringify({ version: '0.1.0', devDependencies: { electron: '44.7.0' } }));
		for (const directory of ['src', 'build', 'scripts', 'assets']) {
			await mkdir(join(root, directory), { recursive: true });
		}
		for (const path of ['package-lock.json', 'product.json', 'tsconfig.json', 'src/tsconfig.json', 'LICENSE.vscode.txt', 'ThirdPartyNotices.vscode.txt']) {
			await put(root, path);
		}
		await put(root, 'assets/tray/trayTemplate.png');
		for (const path of [product.platforms[process.platform].icon, product.platforms[process.platform].fallbackIconSource].filter(Boolean)) {
			await put(root, path.endsWith('.icon') ? `${path}/icon.json` : path, '{}');
		}
		await put(root, 'out/dev-sentinel');
		const factory = (_root, options) => ({
			native: { assertFresh: async () => undefined },
			stop: async () => {
				stopped++;
			},
			run: async () => {
				assert.equal(options.production, true);
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
			}
		});
		const output = await new ReleaseBuild(root, factory).run();
		await new ReleaseVerifier(output).verifyFiles();
		const manifest = JSON.parse(await readFile(join(output, 'build-manifest.json'), 'utf8'));
		assert.equal(manifest.profile, 'release');
		assert.equal(JSON.parse(await readFile(join(output, 'app/package.json'), 'utf8')).main, 'main.cjs');
		for (const file of manifest.files) {
			assert.equal(createHash('sha256').update(await readFile(join(output, file.path))).digest('hex'), file.sha256);
		}
		const previous = await readFile(join(output, 'build-manifest.json'), 'utf8');
		fail = true;
		await assert.rejects(new ReleaseBuild(root, factory).run(), /Test bundles/);
		assert.equal(await readFile(join(output, 'build-manifest.json'), 'utf8'), previous);
		assert.equal(await readFile(join(root, 'out/dev-sentinel'), 'utf8'), 'fixture');
		assert.deepEqual(await readdir(join(root, '.build/release')), [`${process.platform}-${process.arch}`]);
		fail = false;
		mutate = true;
		await assert.rejects(new ReleaseBuild(root, factory).run(), /inputs changed/);
		assert.equal(await readFile(join(output, 'build-manifest.json'), 'utf8'), previous);
		assert.equal(stopped, 3);
		await access(join(output, 'app/assets/tray/trayTemplate.png'));
		await put(output, 'app/main.cjs', 'modified');
		await assert.rejects(new ReleaseVerifier(output).verifyFiles(), /inventory differs/);
		await mkdir(`${output}.lock`);
		await put(`${output}.lock`, 'owner', 'another-build');
		await assert.rejects(new ReleaseBuild(root, factory).run(), /EEXIST/);
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
