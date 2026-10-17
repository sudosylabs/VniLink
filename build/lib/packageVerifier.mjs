/** Inventories complete Electron bundles, preserving framework symlinks as links. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, readFile, readdir, readlink } from 'node:fs/promises';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { packagedLayout } from './packageConfiguration.mjs';

export async function fileSha256(path) {
	const hash = createHash('sha256');
	for await (const bytes of createReadStream(path)) {
		hash.update(bytes);
	}
	return hash.digest('hex');
}

export async function packageInventory(directory, current = directory) {
	const files = [];
	for (const entry of await readdir(current, { withFileTypes: true })) {
		if (current === directory && entry.name === 'packaging-manifest.json') {
			continue;
		}
		const filename = join(current, entry.name);
		const path = relative(directory, filename).split('\\').join('/');
		if (entry.isDirectory()) {
			files.push(...await packageInventory(directory, filename));
		} else if (entry.isSymbolicLink()) {
			const target = await readlink(filename);
			const destination = relative(directory, resolve(dirname(filename), target));
			if (isAbsolute(target) || destination === '..' || destination.startsWith('../') || destination.startsWith('..\\') || isAbsolute(destination)) {
				throw new Error(`Packaged symlink escapes the application: ${path}`);
			}
			files.push({ path, type: 'link', target });
		} else if (entry.isFile()) {
			const stat = await lstat(filename);
			files.push({ path, type: 'file', bytes: stat.size, executable: (stat.mode & 0o111) !== 0, sha256: await fileSha256(filename) });
		} else {
			throw new Error(`Unsupported packaged file: ${path}`);
		}
	}
	return files.sort((a, b) => a.path.localeCompare(b.path, 'en'));
}

export class PackageVerifier {
	constructor(directory, product) {
		this.directory = directory;
		this.product = product;
	}

	async verifyFiles() {
		const manifest = JSON.parse(await readFile(join(this.directory, 'packaging-manifest.json'), 'utf8'));
		assert.equal(manifest.appId, this.product.appId);
		assert.equal(manifest.platform, process.platform);
		assert.equal(manifest.architecture, process.arch);
		assert.equal(manifest.signing, 'unsigned');
		assert.deepEqual(await packageInventory(this.directory), manifest.files, 'Assembled application inventory differs');
		const layout = packagedLayout(this.directory, process.platform, this.product);
		assert.equal(await fileSha256(layout.asar), manifest.asarSha256);
		assert.equal(await fileSha256(layout.native), manifest.nativeSha256);
		return manifest;
	}
}
