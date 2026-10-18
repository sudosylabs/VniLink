/** Inventories and checks every build artifact: the release payload, the assembled application and installers. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { cp, lstat, mkdir, mkdtemp, readFile, readdir, readlink, rm, stat, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { basename, dirname, extname, isAbsolute, join, relative, resolve } from 'node:path';
import { installerExtensions } from './installers.ts';
import { packagedLayout } from './package.ts';
import { runPackagedApplication } from './packagedApplication.ts';
import { ProcessRunner } from './processes.ts';
import { readProduct, type Product } from './product.ts';

export interface ReleaseFile {
	readonly path: string;
	readonly bytes?: number;
	readonly sha256: string;
}

export interface ReleaseManifest {
	readonly source: string;
	readonly product: string;
	readonly version: string;
	readonly electronVersion: string;
	readonly platform: NodeJS.Platform;
	readonly architecture: string;
	readonly rustTarget: string;
	readonly profile: 'release';
	readonly files: readonly ReleaseFile[];
}

export type PackageFile = { readonly path: string; readonly type: 'link'; readonly target: string } | { readonly path: string; readonly type: 'file'; readonly bytes: number; readonly executable: boolean; readonly sha256: string };

export interface PackageManifest {
	readonly appId: string;
	readonly version: string;
	readonly electronVersion: string;
	readonly packagerVersion: string;
	readonly platform: NodeJS.Platform;
	readonly architecture: string;
	readonly signing: 'unsigned';
	readonly executable: string;
	readonly asarSha256: string;
	readonly nativeSha256: string;
	readonly files: readonly PackageFile[];
}

export interface InstallerArtifact {
	readonly path: string;
	readonly bytes: number;
	readonly sha256: string;
}

export interface InstallerManifest {
	readonly application: PackageManifest;
	readonly artifacts: readonly InstallerArtifact[];
}

export async function fileSha256(path: string): Promise<string> {
	const hash = createHash('sha256');
	for await (const bytes of createReadStream(path)) {
		hash.update(bytes as Buffer);
	}
	return hash.digest('hex');
}

function portablePath(path: string): string {
	return path.split('\\').join('/');
}

/** Every regular file under `directory` with its size and checksum; anything else is refused. */
export async function releaseInventory(directory: string, current = directory): Promise<ReleaseFile[]> {
	const files: ReleaseFile[] = [];
	for (const entry of await readdir(current, { withFileTypes: true })) {
		const filename = join(current, entry.name);
		if (entry.isDirectory()) {
			files.push(...await releaseInventory(directory, filename));
		} else if (entry.isFile()) {
			const bytes = await readFile(filename);
			files.push({ path: portablePath(relative(directory, filename)), bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
		} else {
			throw new Error(`Release payload contains an unsupported file type: ${filename}`);
		}
	}
	return files.sort((left, right) => left.path.localeCompare(right.path, 'en'));
}

/** A complete Electron bundle, preserving framework symlinks as links. */
export async function packageInventory(directory: string, current = directory): Promise<PackageFile[]> {
	const files: PackageFile[] = [];
	for (const entry of await readdir(current, { withFileTypes: true })) {
		if (current === directory && entry.name === 'packaging-manifest.json') {
			continue;
		}
		const filename = join(current, entry.name);
		const path = portablePath(relative(directory, filename));
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
			const metadata = await lstat(filename);
			files.push({ path, type: 'file', bytes: metadata.size, executable: (metadata.mode & 0o111) !== 0, sha256: await fileSha256(filename) });
		} else {
			throw new Error(`Unsupported packaged file: ${path}`);
		}
	}
	return files.sort((a, b) => a.path.localeCompare(b.path, 'en'));
}

/** Checks the exact staged payload against its build manifest. */
export async function verifyReleaseFiles(directory: string): Promise<ReleaseManifest> {
	const manifest = JSON.parse(await readFile(join(directory, 'build-manifest.json'), 'utf8')) as ReleaseManifest;
	assert.equal(manifest.profile, 'release', 'Expected a release profile');
	assert.equal(manifest.platform, process.platform, 'Release platform differs from the verifier host');
	assert.equal(manifest.architecture, process.arch, 'Release architecture differs from the verifier host');
	const actual = (await releaseInventory(directory)).filter(file => file.path !== 'build-manifest.json');
	assert.deepEqual(actual, manifest.files, 'Release inventory differs: files missing, added or modified');
	return manifest;
}

interface TemporaryEngine {
	execute(command: string): Promise<string>;
}

/** Checks the staged payload, then loads its addon with an isolated temporary identity. */
export async function verifyRelease(root: string): Promise<void> {
	const directory = join(root, '.build/release', `${process.platform}-${process.arch}`);
	await verifyReleaseFiles(directory);
	const addon = createRequire(import.meta.url)(join(directory, 'app/native/vnilink.node')) as { openTemporaryEngine(): Promise<TemporaryEngine> };
	const engine = await addon.openTemporaryEngine();
	try {
		const reply = JSON.parse(await engine.execute(JSON.stringify({ type: 'snapshot' }))) as { type: string; snapshot: { endpoint_id: string; devices: unknown[] } };
		assert.equal(reply.type, 'snapshot');
		assert.match(reply.snapshot.endpoint_id, /^[a-f0-9]{64}$/);
		assert.equal(reply.snapshot.devices.length, 0, 'Temporary release probe unexpectedly has paired devices');
	} finally {
		await engine.execute(JSON.stringify({ type: 'shutdown' }));
	}
	console.log('Release payload verified; staged addon opened and shut down a temporary engine under Node. Packaged Electron/OS behaviour remains a later check.');
}

/** Checks an assembled application against its packaging manifest. */
export async function verifyPackageFiles(directory: string, product: Product): Promise<PackageManifest> {
	const manifest = JSON.parse(await readFile(join(directory, 'packaging-manifest.json'), 'utf8')) as PackageManifest;
	assert.equal(manifest.appId, product.appId);
	assert.equal(manifest.platform, process.platform);
	assert.equal(manifest.architecture, process.arch);
	assert.equal(manifest.signing, 'unsigned');
	assert.deepEqual(await packageInventory(directory), manifest.files, 'Assembled application inventory differs');
	const layout = packagedLayout(directory, process.platform, product);
	assert.equal(await fileSha256(layout.asar), manifest.asarSha256);
	assert.equal(await fileSha256(layout.native), manifest.nativeSha256);
	return manifest;
}

/** Launches the assembled application outside the checkout and checks it starts. */
export async function verifyPackage(root: string, signal: AbortSignal): Promise<void> {
	await runPackagedApplication(root, join(root, 'dist', `${process.platform}-${process.arch}`), signal);
}

/** Checks installer checksums against their manifest and `SHA256SUMS`. */
export async function verifyInstallerFiles(
	directory: string,
	product: Product,
	platform: NodeJS.Platform = process.platform,
	architecture: string = process.arch,
): Promise<InstallerManifest> {
	const manifest = JSON.parse(await readFile(join(directory, 'installer-manifest.json'), 'utf8')) as InstallerManifest;
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

/** Verifies installer checksums and, on macOS, the mounted application payload and its launch. */
export async function verifyInstallers(root: string, signal: AbortSignal): Promise<void> {
	const product = await readProduct(root);
	const directory = join(root, 'dist/installers', `${process.platform}-${process.arch}`);
	const manifest = await verifyInstallerFiles(directory, product);
	if (process.platform !== 'darwin') {
		console.log('Installer checksums passed. Native installation, upgrade and uninstall checks have not been performed by this command.');
		return;
	}
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vnilink-dmg-check-'));
	const mount = join(work, 'volume');
	let mounted = false;
	try {
		const dmg = join(directory, manifest.artifacts[0].path);
		await processes.run('hdiutil', ['verify', dmg], { signal: AbortSignal.timeout(120000) });
		await mkdir(mount);
		await processes.run('hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, dmg], { signal: AbortSignal.timeout(30000) });
		mounted = true;
		const name = `${product.nameLong}.app`;
		const prefix = `${name}/`;
		const expected = manifest.application.files
			.filter(file => file.path.startsWith(prefix))
			.map(file => ({ ...file, path: file.path.slice(prefix.length) }));
		assert(expected.length > 0, 'Installer manifest has no application payload');
		assert.deepEqual(await packageInventory(join(mount, name)), expected, 'DMG application differs from the assembled input');
		const copy = join(work, 'application');
		await mkdir(copy);
		await cp(join(mount, name), join(copy, name), { recursive: true, verbatimSymlinks: true });
		await writeFile(join(copy, 'packaging-manifest.json'), JSON.stringify(manifest.application));
		await runPackagedApplication(root, copy, signal);
		console.log('DMG integrity, exact application payload and isolated launch passed.');
	} finally {
		await processes.stop();
		// Detach uses a separate owner because cancellation closes the work runner.
		const cleanup = new ProcessRunner();
		try {
			if (mounted) {
				await cleanup.run('hdiutil', ['detach', mount], { signal: AbortSignal.timeout(15000) });
			}
			await rm(work, { recursive: true, force: true });
		} finally {
			await cleanup.stop();
		}
	}
}
