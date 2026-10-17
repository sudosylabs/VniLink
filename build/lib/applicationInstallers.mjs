/** Owns installer generation, input integrity, cancellation and publication. */
import assert from 'node:assert/strict';
import { access, mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { extname, join } from 'node:path';
import { validateProduct } from '../product.mjs';
import { ProcessRunner } from './processes.mjs';
import { ReleaseVerifier } from './releaseVerifier.mjs';
import { PackageVerifier, fileSha256 } from './packageVerifier.mjs';
import { assemblyEnvironment } from './packageConfiguration.mjs';
import { installerConfiguration, installerExtensions } from './installerConfiguration.mjs';

export class ApplicationInstallers {
	#stopped = false;

	constructor(root, processes = new ProcessRunner()) {
		this.root = root;
		this.processes = processes;
	}

	async run() {
		const product = validateProduct(JSON.parse(await readFile(join(this.root, 'product.json'), 'utf8')));
		const target = `${process.platform}-${process.arch}`;
		const stage = join(this.root, '.build/release', target);
		const application = join(this.root, 'dist', target);
		const output = join(this.root, 'dist/installers', target);
		await mkdir(join(this.root, 'dist/installers'), { recursive: true });
		// Both assembly and installer jobs own this lock while reading the application.
		const lock = `${application}.lock`;
		await mkdir(lock);
		let work;
		try {
			const release = await new ReleaseVerifier(stage).verifyFiles();
			const verifier = new PackageVerifier(application, product);
			const manifest = await verifier.verifyFiles();
			assert.equal(manifest.version, release.version);
			assert.equal(manifest.electronVersion, release.electronVersion);
			work = await mkdtemp(join(this.root, 'dist/installers', '.installer-'));
			const buildOutput = join(work, 'output');
			const configuration = installerConfiguration(this.root, stage, buildOutput, product, manifest);
			const configPath = join(work, 'builder.json');
			await writeFile(configPath, JSON.stringify(configuration, null, 2));
			const prepackaged = process.platform === 'darwin' ? join(application, `${product.nameLong}.app`) : application;
			const cli = createRequire(import.meta.url).resolve('electron-builder/cli.js');
			await this.processes.run(process.execPath, [cli, '--config', configPath, '--prepackaged', prepackaged, { darwin: '--mac', win32: '--win', linux: '--linux' }[process.platform], `--${process.arch}`, '--publish', 'never'], { cwd: this.root, env: assemblyEnvironment(process.env) });
			const artifacts = [];
			for (const extension of installerExtensions(process.platform)) {
				const entries = (await readdir(buildOutput, { withFileTypes: true })).filter(entry => entry.isFile() && extname(entry.name) === extension);
				assert.equal(entries.length, 1, `Expected exactly one ${extension} artifact`);
				const path = entries[0].name;
				const bytes = (await stat(join(buildOutput, path))).size;
				assert(bytes > 0);
				artifacts.push({ path, bytes, sha256: await fileSha256(join(buildOutput, path)) });
			}
			await verifier.verifyFiles();
			await new ReleaseVerifier(stage).verifyFiles();
			await writeFile(join(buildOutput, 'installer-manifest.json'), JSON.stringify({ application: manifest, artifacts }, null, 2) + '\n');
			await writeFile(join(buildOutput, 'SHA256SUMS'), artifacts.map(file => `${file.sha256}  ${file.path}\n`).join(''));
			if (this.#stopped) {
				throw new Error('Installer generation cancelled');
			}
			// Restore the last completed output if publication fails.
			const previous = join(work, 'previous');
			let hadPrevious = false;
			try {
				await access(output);
				await rename(output, previous);
				hadPrevious = true;
			} catch (error) {
				if (error.code !== 'ENOENT') {
					throw error;
				}
			}
			try {
				await rename(buildOutput, output);
			} catch (error) {
				if (hadPrevious) {
					await rename(previous, output);
				}
				throw error;
			}
			console.log(`Unsigned installers generated: ${output}`);
			return output;
		} finally {
			try {
				await this.processes.stop();
			} finally {
				if (work) {
					await rm(work, { recursive: true, force: true });
				}
				await rm(lock, { recursive: true, force: true });
			}
		}
	}

	stop() {
		this.#stopped = true;
		return this.processes.stop();
	}
}
