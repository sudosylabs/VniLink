/** Produces a clean application payload; Electron assembly/signing/installers are later stages. */
import { createHash } from 'node:crypto';
import { cp, mkdir, mkdtemp, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join, relative } from 'node:path';
import { buildOptions } from '../tasks.mjs';
import { loadProduct } from '../product.mjs';
import { ApplicationBuild } from './applicationBuild.mjs';

export async function releaseInventory(directory, current = directory) {
	const files = [];
	for (const entry of await readdir(current, { withFileTypes: true })) {
		const filename = join(current, entry.name);
		if (entry.isDirectory()) {
			files.push(...await releaseInventory(directory, filename));
		} else if (entry.isFile()) {
			const bytes = await readFile(filename);
			files.push({ path: relative(directory, filename).split('\\').join('/'), bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
		} else {
			throw new Error(`Release payload contains an unsupported file type: ${filename}`);
		}
	}
	return files.sort((left, right) => left.path.localeCompare(right.path, 'en'));
}

async function sourceFingerprint(root) {
	const files = [];
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

export class ReleaseBuild {
	#builder;
	#stopped = false;

	constructor(root, createBuild = (directory, options) => new ApplicationBuild(directory, options)) {
		this.root = root;
		this.createBuild = createBuild;
	}

	async run() {
		const product = await loadProduct();
		const platform = product.platforms[process.platform];
		const target = platform?.targets.find(value => value.architecture === process.arch);
		if (!target) {
			throw new Error(`No approved release target for ${process.platform}-${process.arch}`);
		}
		if (process.env.CARGO_BUILD_TARGET && process.env.CARGO_BUILD_TARGET !== target.rustTarget) {
			throw new Error('CARGO_BUILD_TARGET differs from the approved host release target');
		}
		const parent = join(this.root, '.build/release');
		const output = join(parent, `${process.platform}-${process.arch}`);
		await mkdir(parent, { recursive: true });
		const lock = `${output}.lock`;
		await mkdir(lock); // Refuse overlapping publishers rather than deleting another build's files.
		let work;
		try {
			work = await mkdtemp(join(parent, '.stage-'));
			const app = join(work, 'app');
			const source = await sourceFingerprint(this.root);
			this.#builder = this.createBuild(this.root, { production: true, outputDirectory: app, target: target.rustTarget, nativeEnvironment: process.platform === 'darwin' ? { MACOSX_DEPLOYMENT_TARGET: platform.minimumSystemVersion } : {} });
			if (this.#stopped) {
				throw new Error('Release build stopped');
			}
			await this.#builder.run();
			if (this.#stopped) {
				throw new Error('Release build stopped');
			}
			const packageJson = JSON.parse(await readFile(join(this.root, 'package.json'), 'utf8'));
			await writeFile(join(app, 'package.json'), JSON.stringify({ name: product.applicationName, productName: product.nameLong, version: packageJson.version, private: true, main: 'main.cjs', type: 'module', author: product.publisherName, description: 'Cross-platform remote access through paired computers.', homepage: product.repositoryUrl, desktopName: product.platforms.linux.desktopFileName, license: 'UNLICENSED' }, null, 2) + '\n');
			await writeFile(join(app, 'product.json'), JSON.stringify(product, null, 2) + '\n');
			await cp(join(this.root, 'assets/tray'), join(app, 'assets/tray'), { recursive: true });
			for (const asset of [platform.icon, platform.fallbackIconSource].filter(Boolean)) {
				const destination = join(work, 'packaging-resources', asset);
				await mkdir(join(destination, '..'), { recursive: true });
				await cp(join(this.root, asset), destination, { recursive: true });
			}
			await this.#builder.native.assertFresh(app);
			if (source !== await sourceFingerprint(this.root)) {
				throw new Error('Application/build inputs changed during release build; rebuild before packaging');
			}
			const files = await releaseInventory(work);
			const paths = new Set(files.map(file => file.path));
			for (const name of [...buildOptions(this.root, { production: true, outputDirectory: app }).map(option => relative(work, option.outfile).split('\\').join('/')), 'app/renderer/renderer.css', 'app/renderer/index.html', 'app/native/vnilink.node', 'app/native/build.json', 'app/licenses/native-codecs/native-codecs.txt', 'app/relink/source/Cargo.lock']) {
				if (!paths.has(name)) {
					throw new Error(`Missing release input: ${name}`);
				}
			}
			if (paths.has('app/smoke.cjs') || paths.has('app/browser-tests.js')) {
				throw new Error('Test bundles must not be distributed');
			}
			await writeFile(join(work, 'build-manifest.json'), JSON.stringify({ source, product: product.appId, version: packageJson.version, electronVersion: packageJson.devDependencies.electron, platform: process.platform, architecture: process.arch, rustTarget: target.rustTarget, profile: 'release', files }, null, 2) + '\n');
			if (this.#stopped) {
				throw new Error('Release build stopped');
			}
			await rm(output, { recursive: true, force: true });
			await rename(work, output);
			work = undefined;
			console.log(`Release application staged: ${output}`);
			return output;
		} finally {
			await this.#builder?.stop();
			if (work) {
				await rm(work, { recursive: true, force: true });
			}
			await rm(lock, { recursive: true, force: true });
		}
	}

	async stop() {
		this.#stopped = true;
		await this.#builder?.stop();
	}
}
