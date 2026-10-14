import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, writeFile, readFile, rename, rm, access } from 'node:fs/promises';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { build } from 'esbuild';
import { watchApplication } from '../../build/watch.mjs';

async function until(predicate) {
	const deadline = Date.now() + 10000;
	while (!await predicate()) {
		assert.ok(Date.now() < deadline, 'Watcher did not emit the updated file');
		await new Promise(resolve => setTimeout(resolve, 30));
	}
}

test('desktop watcher rebuilds TS/CSS, handles atomic resource saves and leaves Rust alone', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-watch-'));
	async function put(path, text) {
		const filename = join(root, path);
		await mkdir(dirname(filename), { recursive: true });
		await writeFile(filename, text);
	}
	const paths = ['src/main.ts', 'src/preload.ts', 'src/platform/engine/electron-utility/engineHost.ts', 'test/smoke/main.ts', 'test/browser/icons.ts', 'test/browser/dataChannels.ts', 'test/browser/tabs.ts'];
	let watcher;
	try {
		await put('tsconfig.json', '{}');
		for (const path of paths) {
			await put(path, 'console.log("original");');
		}
		await put('src/renderer.ts', 'import "./style.css"; console.log("original");');
		await put('src/style.css', 'body { color: red; }');
		await put('src/workbench/browser/index.html', '<title>original</title>');
		await put('assets/desktop/app-icon.svg', '<svg/>');
		await put('node_modules/@phosphor-icons/core/LICENSE', 'phosphor');
		await put('LICENSE.vscode.txt', 'MIT');
		await put('ThirdPartyNotices.vscode.txt', 'notices');
		await put('out/native/vnilink.node', 'native-sentinel');
		watcher = await watchApplication(root);
		await put('src/renderer.ts', 'import "./style.css"; console.log("updated-renderer");');
		await put('src/style.css', 'body { color: blue; }');
		await put('src/main.ts', 'console.log("updated-main");');
		await until(async () => (await readFile(join(root, 'out/renderer/renderer.js'), 'utf8')).includes('updated-renderer') && (await readFile(join(root, 'out/renderer/renderer.css'), 'utf8')).includes('color: blue') && (await readFile(join(root, 'out/main.cjs'), 'utf8')).includes('updated-main'));
		await put('src/workbench/browser/replacement.html', '<title>atomic-save</title>');
		await rename(join(root, 'src/workbench/browser/replacement.html'), join(root, 'src/workbench/browser/index.html'));
		await until(async () => (await readFile(join(root, 'out/renderer/index.html'), 'utf8')).includes('atomic-save'));
		await rm(join(root, 'src/workbench/browser/index.html'));
		await until(async () => {
			try {
				await access(join(root, 'out/renderer/index.html'));
				return false;
			} catch {
				return true;
			}
		});
		await put('src/workbench/browser/index.html', '<title>restored</title>');
		await until(async () => {
			try {
				return (await readFile(join(root, 'out/renderer/index.html'), 'utf8')).includes('restored');
			} catch {
				return false;
			}
		});
		await put('crates/example/src/lib.rs', 'this deliberately is not valid Rust');
		assert.equal(await readFile(join(root, 'out/native/vnilink.node'), 'utf8'), 'native-sentinel');
		await watcher.dispose();
		watcher = undefined;
		await put('src/main.ts', 'console.log("after-disposal");');
		await new Promise(resolve => setTimeout(resolve, 300));
		assert.equal((await readFile(join(root, 'out/main.cjs'), 'utf8')).includes('after-disposal'), false);
	} finally {
		await watcher?.dispose();
		await rm(root, { recursive: true, force: true });
	}
});

test('Reload Window shortcut is enabled only in unpackaged development launches', async () => {
	const output = await build({
		entryPoints: ['src/app/electron-main/development.ts'], bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{
			name: 'electron-fixture', setup(builder) {
				builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
				builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const app = { get isPackaged() { return process.env.VNILINK_TEST_PACKAGED === \'1\'; } };', loader: 'js' }));
			}
		}]
	});
	const { registerDevelopmentWindow } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
	const previous = process.env.VNILINK_DEV;
	const previousPackaged = process.env.VNILINK_TEST_PACKAGED;
	let handler;
	let reloaded = 0;
	let prevented = 0;
	const window = {
		webContents: {
			on: (_name, callback) => {
				handler = callback;
			}, reloadIgnoringCache: () => reloaded++
		}
	};
	try {
		delete process.env.VNILINK_DEV;
		registerDevelopmentWindow(window);
		assert.equal(handler, undefined);
		process.env.VNILINK_DEV = '1';
		process.env.VNILINK_TEST_PACKAGED = '1';
		registerDevelopmentWindow(window);
		assert.equal(handler, undefined);
		delete process.env.VNILINK_TEST_PACKAGED;
		registerDevelopmentWindow(window);
		const event = { preventDefault: () => prevented++ };
		const input = { type: 'keyDown', key: 'r', meta: process.platform === 'darwin', control: process.platform !== 'darwin', alt: false, shift: false };
		handler(event, { ...input, meta: false, control: false });
		handler(event, { ...input, alt: true });
		assert.equal(reloaded, 0);
		handler(event, input);
		assert.equal(reloaded, 1);
		assert.equal(prevented, 1);
	} finally {
		if (previousPackaged === undefined) {
			delete process.env.VNILINK_TEST_PACKAGED;
		} else {
			process.env.VNILINK_TEST_PACKAGED = previousPackaged;
		}
		if (previous === undefined) {
			delete process.env.VNILINK_DEV;
		} else {
			process.env.VNILINK_DEV = previous;
		}
	}
});
