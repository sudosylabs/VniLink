/** Produces a clean application payload; Electron assembly, signing and installers are later stages. */
import { createHash } from 'node:crypto';
import { cp, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join, relative } from 'node:path';
import { buildApplication, bundleOptions, type ApplicationBuildOptions } from './bundles.ts';
import { assertNativeFresh, type NativeOptions } from './native.ts';
import { loadProduct } from './product.ts';
import { releaseInventory, type ReleaseFile } from './verify.ts';

async function sourceFingerprint(root: string): Promise<string> {
	const files: ReleaseFile[] = [];
	for (const directory of ['src', 'build', 'scripts', 'assets']) {
		for (const entry of await releaseInventory(join(root, directory))) {
			files.push({ ...entry, path: `${directory}/${entry.path}` });
		}
	}
	for (const path of ['package.json', 'package-lock.json', 'product.json', 'tsconfig.json', 'src/tsconfig.json', 'LICENSE.vscode.txt', 'ThirdPartyNotices.vscode.txt']) {
		files.push({ path, sha256: createHash('sha256').update(await readFile(join(root, path))).digest('hex') });
	}
	return createHash('sha256').update(JSON.stringify(files)).digest('hex');
}

export interface ReleaseOptions {
	/** Builds the application payload; replaced by tests. */
	readonly buildApplication?: (root: string, signal: AbortSignal, options: ApplicationBuildOptions) => Promise<void>;
	/** Checks the staged addon; replaced by tests. */
	readonly assertNativeFresh?: (root: string, outputDirectory: string, options: NativeOptions) => Promise<void>;
}

interface PackageJson {
	readonly version: string;
	readonly devDependencies: { readonly electron: string };
}

/** Stages `.build/release/<platform>-<arch>` with its inventory; refuses to overlap another release build. */
export async function buildRelease(root: string, signal: AbortSignal, options: ReleaseOptions = {}): Promise<string> {
	const product = await loadProduct(root);
	const platform = process.platform === 'darwin' || process.platform === 'win32' || process.platform === 'linux' ? product.platforms[process.platform] : undefined;
	const target = platform?.targets.find(value => value.architecture === process.arch);
	if (!platform || !target) {
		throw new Error(`No approved release target for ${process.platform}-${process.arch}`);
	}
	if (process.env.CARGO_BUILD_TARGET && process.env.CARGO_BUILD_TARGET !== target.rustTarget) {
		throw new Error('CARGO_BUILD_TARGET differs from the approved host release target');
	}
	const parent = join(root, '.build/release');
	const output = join(parent, `${process.platform}-${process.arch}`);
	await mkdir(parent, { recursive: true });
	const lock = `${output}.lock`;
	await mkdir(lock); // Refuse overlapping publishers rather than deleting another build's files.
	let work: string | undefined;
	try {
		const stage = await mkdtemp(join(parent, '.stage-'));
		work = stage;
		const app = join(stage, 'app');
		const source = await sourceFingerprint(root);
		const minimum = process.platform === 'darwin' ? { minimumSystemVersion: product.platforms.darwin.minimumSystemVersion } : {};
		const native: NativeOptions = { profile: 'release', target: target.rustTarget, ...minimum };
		await (options.buildApplication ?? buildApplication)(root, signal, { production: true, outputDirectory: app, native });
		signal.throwIfAborted();
		const packageJson = JSON.parse(await readFile(join(root, 'package.json'), 'utf8')) as PackageJson;
		await writeFile(join(app, 'package.json'), JSON.stringify({ name: product.applicationName, productName: product.nameLong, version: packageJson.version, private: true, main: 'main.cjs', type: 'module', author: product.publisherName, description: 'Cross-platform remote access through paired computers.', homepage: product.repositoryUrl, desktopName: product.platforms.linux.desktopFileName, license: 'UNLICENSED' }, null, 2) + '\n');
		await writeFile(join(app, 'product.json'), JSON.stringify(product, null, 2) + '\n');
		const artwork = process.platform === 'darwin' ? [platform.icon, product.platforms.darwin.fallbackIconSource] : [platform.icon];
		for (const asset of artwork) {
			const destination = join(stage, 'packaging-resources', asset);
			await mkdir(join(destination, '..'), { recursive: true });
			await cp(join(root, asset), destination, { recursive: true });
		}
		await (options.assertNativeFresh ?? assertNativeFresh)(root, app, native);
		if (source !== await sourceFingerprint(root)) {
			throw new Error('Application/build inputs changed during release build; rebuild before packaging');
		}
		const files = await releaseInventory(stage);
		const paths = new Set(files.map(file => file.path));
		const bundles = bundleOptions(root, { production: true, outputDirectory: app }).map(option => relative(stage, option.outfile ?? '').split('\\').join('/'));
		for (const name of [...bundles, 'app/renderer/renderer.css', 'app/renderer/index.html', 'app/assets/tray/idleTemplate.png', 'app/native/vnilink.node', 'app/native/build.json', 'app/licenses/native-codecs/native-codecs.txt', 'app/relink/source/Cargo.lock']) {
			if (!paths.has(name)) {
				throw new Error(`Missing release input: ${name}`);
			}
		}
		if (paths.has('app/smoke.cjs') || paths.has('app/browser-tests.js')) {
			throw new Error('Test bundles must not be distributed');
		}
		await writeFile(join(stage, 'build-manifest.json'), JSON.stringify({ source, product: product.appId, version: packageJson.version, electronVersion: packageJson.devDependencies.electron, platform: process.platform, architecture: process.arch, rustTarget: target.rustTarget, profile: 'release', files }, null, 2) + '\n');
		signal.throwIfAborted();
		await rm(output, { recursive: true, force: true });
		await rename(stage, output);
		work = undefined;
		console.log(`Release application staged: ${output}`);
		return output;
	} finally {
		if (work) {
			await rm(work, { recursive: true, force: true });
		}
		await rm(lock, { recursive: true, force: true });
	}
}
