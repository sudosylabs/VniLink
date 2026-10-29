import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { nodeVersionAccepted } from '../../build/lib/prerequisites.ts';
import { computeState, isUpToDate, saveState } from '../../build/npm/installState.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('Node versions follow .nvmrc as VS Code does, and every place that pins Node agrees', async () => {
	assert(nodeVersionAccepted('22.19.0', '22.19.0'));
	assert(nodeVersionAccepted('22.20.1', '22.19.0'));
	assert(nodeVersionAccepted('v22.19.3', '22.19.0'));
	assert(!nodeVersionAccepted('22.18.9', '22.19.0'), 'type stripping without a flag needs 22.18; the repository pins 22.19');
	assert(!nodeVersionAccepted('20.19.0', '22.19.0'));
	assert(!nodeVersionAccepted('24.11.0', '22.19.0'), 'another major is refused, as in VS Code');
	assert(!nodeVersionAccepted('22.19.0', ''));
	const nvmrc = (await readFile(join(root, '.nvmrc'), 'utf8')).trim();
	const packageJson = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
	assert.equal(packageJson.engines.node, `^${nvmrc}`, 'npm (engine-strict) and preinstall must accept the same versions');
	assert.equal(packageJson.volta.node, nvmrc, 'Volta pins the version nvm reads from .nvmrc');
	assert.match(await readFile(join(root, '.npmrc'), 'utf8'), /^engine-strict=true$/m, 'npm must refuse a Node too old to run the TypeScript preinstall');
	const dockerNode = /FROM node:(\d+\.\d+\.\d+)/.exec(await readFile(join(root, 'build/containers/Dockerfile.linux'), 'utf8'))?.[1];
	assert(dockerNode && nodeVersionAccepted(dockerNode, nvmrc), `the Linux image's Node ${dockerNode} must satisfy .nvmrc ${nvmrc}`);
});

test('postinstall skips work whose inputs are unchanged, as VS Code does', async () => {
	const work = await mkdtemp(join(tmpdir(), 'vnilink-install-state-'));
	try {
		await mkdir(join(work, 'node_modules'));
		await writeFile(join(work, 'package.json'), JSON.stringify({ name: 'x', scripts: { a: '1' }, devDependencies: { b: '1' } }));
		await writeFile(join(work, '.nvmrc'), '22.19.0\n');
		assert.equal(await isUpToDate(work), false);
		await saveState(work);
		assert.equal(await isUpToDate(work), true);
		await writeFile(join(work, 'package.json'), JSON.stringify({ name: 'x', scripts: { a: '2' }, devDependencies: { b: '1' } }));
		assert.equal(await isUpToDate(work), true, 'scripts do not change what is installed');
		await writeFile(join(work, 'package.json'), JSON.stringify({ name: 'x', scripts: { a: '2' }, devDependencies: { b: '2' } }));
		assert.equal(await isUpToDate(work), false);
		await saveState(work);
		await writeFile(join(work, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.92.0"\n');
		assert.equal(await isUpToDate(work), false, 'a new toolchain is prepared again');
		assert.equal((await computeState(work)).nodeVersion, process.versions.node);
	} finally {
		await rm(work, { recursive: true, force: true });
	}
});

test('preinstall and the install state import only Node modules: they run before the dependencies matter', async () => {
	for (const file of ['build/npm/preinstall.ts', 'build/npm/installState.ts']) {
		const source = await readFile(join(root, file), 'utf8');
		for (const [, specifier] of source.matchAll(/^import (?!type )[^;]*from '([^']+)'/gm)) {
			assert(specifier.startsWith('node:') || specifier === '../lib/prerequisites.ts', `${file} imports ${specifier}`);
		}
	}
	const packageJson = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
	assert.equal(packageJson.scripts.preinstall, 'node build/npm/preinstall.ts');
	assert.equal(packageJson.scripts.postinstall, 'node build/npm/postinstall.ts');
});
