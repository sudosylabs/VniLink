/** Assembles one host-native, unsigned Electron directory from a verified release payload. */
import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { access, mkdir, mkdtemp, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, relative } from 'node:path';
import { ReleaseVerifier } from './releaseVerifier.mjs';
import { ProcessRunner } from './processes.mjs';
import { validateProduct } from '../product.mjs';
import { assemblyEnvironment, packageConfiguration, packagedLayout } from './packageConfiguration.mjs';
import { packageInventory } from './packageVerifier.mjs';

export class ApplicationPackage {
	#stopped = false;

	constructor(root) {
		this.root = root;
		this.processes = new ProcessRunner();
	}

	async run() {
		const stage = join(this.root, '.build/release', `${process.platform}-${process.arch}`);
		const manifest = await new ReleaseVerifier(stage).verifyFiles();
		const product = validateProduct(JSON.parse(await readFile(join(stage, 'app/product.json'), 'utf8')));
		const electron = JSON.parse(await readFile(join(this.root, 'node_modules/electron/package.json'), 'utf8'));
		if (manifest.electronVersion !== electron.version) {
			throw new Error('Installed Electron differs from the staged release version');
		}
		const parent = join(this.root, 'dist');
		const output = join(parent, `${process.platform}-${process.arch}`);
		await mkdir(parent, { recursive: true });
		const lock = `${output}.lock`;
		await mkdir(lock);
		let work;
		try {
			work = await mkdtemp(join(parent, '.assemble-'));
			const buildOutput = join(work, 'output');
			const configuration = packageConfiguration(this.root, stage, buildOutput, product, manifest);
			const configPath = join(work, 'builder.json');
			await writeFile(configPath, JSON.stringify(configuration, null, 2));
			const cli = createRequire(import.meta.url).resolve('electron-builder/cli.js');
			const platform = { darwin: '--mac', win32: '--win', linux: '--linux' }[process.platform];
			await this.processes.run(process.execPath, [cli, '--config', configPath, '--dir', platform, `--${process.arch}`, '--publish', 'never'], { cwd: this.root, env: assemblyEnvironment(process.env) });
			const candidates = [];
			for (const entry of await readdir(buildOutput, { withFileTypes: true })) {
				if (entry.isDirectory()) {
					const directory = join(buildOutput, entry.name);
					try {
						await access(packagedLayout(directory, process.platform, product).executable, constants.X_OK);
						candidates.push(directory);
					} catch (error) {
						if (error.code !== 'ENOENT') {
							throw error;
						}
					}
				}
			}
			if (candidates.length !== 1) {
				throw new Error('Expected exactly one assembled Electron application');
			}
			const directory = candidates[0];
			const layout = packagedLayout(directory, process.platform, product);
			const native = await readFile(layout.native);
			const nativeInput = manifest.files.find(file => file.path === 'app/native/vnilink.node');
			if (createHash('sha256').update(native).digest('hex') !== nativeInput?.sha256) {
				throw new Error('Packager changed or omitted the staged native addon');
			}
			await access(join(layout.resources, 'licenses/native-codecs/native-codecs.txt'));
			await access(join(layout.resources, 'relink/source/Cargo.lock'));
			const packager = JSON.parse(await readFile(join(this.root, 'node_modules/electron-builder/package.json'), 'utf8'));
			await writeFile(join(directory, 'packaging-manifest.json'), JSON.stringify({ appId: product.appId, version: manifest.version, electronVersion: electron.version, packagerVersion: packager.version, platform: process.platform, architecture: process.arch, signing: 'unsigned', executable: relative(directory, layout.executable).split('\\').join('/'), asarSha256: createHash('sha256').update(await readFile(layout.asar)).digest('hex'), nativeSha256: nativeInput.sha256, files: await packageInventory(directory) }, null, 2) + '\n');
			await new ReleaseVerifier(stage).verifyFiles();
			if (this.#stopped) {
				throw new Error('Application assembly cancelled');
			}
			await rm(output, { recursive: true, force: true });
			await rename(directory, output);
			console.log(`Unsigned Electron application assembled: ${output}`);
			return output;
		} finally {
			await this.processes.stop();
			if (work) {
				await rm(work, { recursive: true, force: true });
			}
			await rm(lock, { recursive: true, force: true });
		}
	}

	stop() {
		this.#stopped = true;
		return this.processes.stop();
	}
}
