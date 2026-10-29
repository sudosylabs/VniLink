/** Installer generation: input integrity, cancellation and publication of the host's installers. */
import assert from 'node:assert/strict';
import { access, mkdir, mkdtemp, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { extname, join } from 'node:path';
import * as darwin from './package.darwin.ts';
import * as linux from './package.linux.ts';
import { assemblyEnvironment, builderPlatformFlag, packageConfiguration, type BuilderConfiguration } from './package.ts';
import * as win32 from './package.win32.ts';
import { ProcessRunner } from './processes.ts';
import { readProduct, type Product } from './product.ts';
import { fileSha256, verifyPackageFiles, verifyReleaseFiles, type InstallerArtifact, type ReleaseManifest } from './verify.ts';

const platforms: Partial<Record<NodeJS.Platform, typeof darwin>> = { darwin, win32, linux };

function platformInstallers(platform: NodeJS.Platform): typeof darwin {
	const installers = platforms[platform];
	if (!installers) {
		throw new Error(`Unsupported installer host: ${platform}`);
	}
	return installers;
}

export function installerExtensions(platform: NodeJS.Platform): readonly string[] {
	return platformInstallers(platform).extensions;
}

/** Native-host installer targets consume an existing, verified Electron application. */
export function installerConfiguration(root: string, stage: string, output: string, product: Product, manifest: Pick<ReleaseManifest, 'platform' | 'electronVersion'>): BuilderConfiguration {
	const base = packageConfiguration(root, stage, output, product, manifest);
	return { ...base, artifactName: product.nameLong + '-${version}-${os}-${arch}.${ext}', ...platformInstallers(manifest.platform).installerOptions(product, base) };
}

export interface InstallerOptions {
	/** Runs electron-builder; replaced by tests. */
	readonly runBuilder?: (processes: ProcessRunner, args: readonly string[], cwd: string) => Promise<unknown>;
}

/** Generates `dist/installers/<platform>-<arch>` from the verified assembled application. */
export async function buildInstallers(root: string, signal: AbortSignal, options: InstallerOptions = {}): Promise<string> {
	const product = await readProduct(root);
	const target = `${process.platform}-${process.arch}`;
	const stage = join(root, '.build/release', target);
	const application = join(root, 'dist', target);
	const output = join(root, 'dist/installers', target);
	await mkdir(join(root, 'dist/installers'), { recursive: true });
	// Both assembly and installer jobs own this lock while reading the application.
	const lock = `${application}.lock`;
	await mkdir(lock);
	const processes = new ProcessRunner(signal);
	let work: string | undefined;
	try {
		const release = await verifyReleaseFiles(stage);
		const manifest = await verifyPackageFiles(application, product);
		assert.equal(manifest.version, release.version);
		assert.equal(manifest.electronVersion, release.electronVersion);
		work = await mkdtemp(join(root, 'dist/installers', '.installer-'));
		const buildOutput = join(work, 'output');
		const configPath = join(work, 'builder.json');
		await writeFile(configPath, JSON.stringify(installerConfiguration(root, stage, buildOutput, product, manifest), null, 2));
		const prepackaged = process.platform === 'darwin' ? join(application, `${product.nameLong}.app`) : application;
		const args = ['--config', configPath, '--prepackaged', prepackaged, builderPlatformFlag(), `--${process.arch}`, '--publish', 'never'];
		await (options.runBuilder ?? runElectronBuilder)(processes, args, root);
		const artifacts: InstallerArtifact[] = [];
		for (const extension of installerExtensions(process.platform)) {
			const entries = (await readdir(buildOutput, { withFileTypes: true })).filter(entry => entry.isFile() && extname(entry.name) === extension);
			assert.equal(entries.length, 1, `Expected exactly one ${extension} artifact`);
			const path = entries[0].name;
			const bytes = (await stat(join(buildOutput, path))).size;
			assert(bytes > 0);
			artifacts.push({ path, bytes, sha256: await fileSha256(join(buildOutput, path)) });
		}
		await verifyPackageFiles(application, product);
		await verifyReleaseFiles(stage);
		await writeFile(join(buildOutput, 'installer-manifest.json'), JSON.stringify({ application: manifest, artifacts }, null, 2) + '\n');
		await writeFile(join(buildOutput, 'SHA256SUMS'), artifacts.map(file => `${file.sha256}  ${file.path}\n`).join(''));
		signal.throwIfAborted();
		await publish(buildOutput, output, join(work, 'previous'));
		console.log(`Unsigned installers generated: ${output}`);
		return output;
	} finally {
		try {
			await processes.stop();
		} finally {
			if (work) {
				await rm(work, { recursive: true, force: true });
			}
			await rm(lock, { recursive: true, force: true });
		}
	}
}

function runElectronBuilder(processes: ProcessRunner, args: readonly string[], cwd: string): Promise<number> {
	const cli = createRequire(import.meta.url).resolve('electron-builder/cli.js');
	return processes.run(process.execPath, [cli, ...args], { cwd, env: assemblyEnvironment(process.env) });
}

/** Replaces `output` with `built`, restoring the last completed output if publication fails. */
async function publish(built: string, output: string, previous: string): Promise<void> {
	let hadPrevious = false;
	try {
		await access(output);
		await rename(output, previous);
		hadPrevious = true;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
			throw error;
		}
	}
	try {
		await rename(built, output);
	} catch (error) {
		if (hadPrevious) {
			await rename(previous, output);
		}
		throw error;
	}
}
