/** Verifies installer checksums and, on macOS, the mounted application payload. */
import assert from 'node:assert/strict';
import { cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { basename, extname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileSha256, packageInventory } from './packageVerifier.mjs';
import { installerExtensions } from './installerConfiguration.mjs';
import { ProcessRunner } from './processes.mjs';

export async function verifyInstallerFiles(directory, product, platform = process.platform, architecture = process.arch) {
	const manifest = JSON.parse(await readFile(join(directory, 'installer-manifest.json'), 'utf8'));
	assert.equal(manifest.application.appId, product.appId);
	assert.equal(manifest.application.platform, platform);
	assert.equal(manifest.application.architecture, architecture);
	assert.equal(manifest.application.signing, 'unsigned');
	assert.deepEqual(manifest.artifacts.map(file => extname(file.path)).sort(), [...installerExtensions(platform)].sort());
	for (const file of manifest.artifacts) {
		assert.equal(file.path, basename(file.path), 'Installer path must be a filename');
		assert(!file.path.includes('\\') && !file.path.includes('/'));
		assert.match(file.sha256, /^[a-f0-9]{64}$/);
		assert(file.bytes > 0);
		assert.equal((await stat(join(directory, file.path))).size, file.bytes);
		assert.equal(await fileSha256(join(directory, file.path)), file.sha256, `Installer checksum differs: ${file.path}`);
	}
	assert.equal(await readFile(join(directory, 'SHA256SUMS'), 'utf8'), manifest.artifacts.map(file => `${file.sha256}  ${file.path}\n`).join(''));
	return manifest;
}

export class InstallerVerifier {
	constructor(root, application) {
		this.root = root;
		this.application = application;
		this.processes = new ProcessRunner();
	}

	async run() {
		const product = JSON.parse(await readFile(join(this.root, 'product.json'), 'utf8'));
		const directory = join(this.root, 'dist/installers', `${process.platform}-${process.arch}`);
		const manifest = await verifyInstallerFiles(directory, product);
		if (process.platform !== 'darwin') {
			console.log('Installer checksums passed. Native installation, upgrade and uninstall checks have not been performed by this command.');
			return;
		}
		const work = await mkdtemp(join(tmpdir(), 'vnilink-dmg-check-'));
		const mount = join(work, 'volume');
		let mounted = false;
		try {
			const dmg = join(directory, manifest.artifacts[0].path);
			await this.processes.run('hdiutil', ['verify', dmg], { signal: AbortSignal.timeout(120000) });
			await mkdir(mount);
			await this.processes.run('hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, dmg], { signal: AbortSignal.timeout(30000) });
			mounted = true;
			const name = `${product.nameLong}.app`;
			const prefix = `${name}/`;
			const expected = manifest.application.files.filter(file => file.path.startsWith(prefix)).map(file => ({ ...file, path: file.path.slice(prefix.length) }));
			assert(expected.length > 0, 'Installer manifest has no application payload');
			assert.deepEqual(await packageInventory(join(mount, name)), expected, 'DMG application differs from the assembled input');
			const copy = join(work, 'application');
			await mkdir(copy);
			await cp(join(mount, name), join(copy, name), { recursive: true, verbatimSymlinks: true });
			await writeFile(join(copy, 'packaging-manifest.json'), JSON.stringify(manifest.application));
			await this.application.run(copy);
			console.log('DMG integrity, exact application payload and isolated launch passed.');
		} finally {
			// Detach uses a separate owner because cancellation closes the work runner.
			const cleanup = new ProcessRunner();
			try {
				if (mounted) {
					await cleanup.run('hdiutil', ['detach', mount], { signal: AbortSignal.timeout(15000) });
				}
				await rm(work, { recursive: true, force: true });
			} finally {
				await cleanup.stop();
				await this.stop();
			}
		}
	}

	async stop() {
		await Promise.all([this.processes.stop(), this.application.stop()]);
	}
}
