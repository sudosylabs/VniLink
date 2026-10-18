/** Assembles one host-native, unsigned Electron directory from a verified release payload. */
import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { access, mkdir, mkdtemp, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, relative } from 'node:path';
import { ProcessRunner } from './processes.ts';
import { validateProduct, type Product, type ProductPlatform } from './product.ts';
import { packageInventory, verifyReleaseFiles, type ReleaseManifest } from './verify.ts';

/** electron-builder v26 options; only what VniLink sets is described. */
export type BuilderConfiguration = Record<string, unknown> & {
	readonly mac: Record<string, unknown>;
	readonly win: Record<string, unknown>;
	readonly linux: Record<string, unknown>;
};

export interface PackagedLayout {
	readonly executable: string;
	readonly resources: string;
	readonly asar: string;
	readonly native: string;
}

/** Converts approved staged product inputs to pinned electron-builder options. */
export function packageConfiguration(root: string, stage: string, output: string, product: Product, manifest: Pick<ReleaseManifest, 'platform' | 'electronVersion'>): BuilderConfiguration {
	const icon = (platform: ProductPlatform): string => join(stage, 'packaging-resources', product.platforms[platform].icon);
	const resources: Record<string, unknown>[] = [
		{ from: join(stage, 'app/licenses'), to: 'licenses', filter: ['**/*'] },
		{ from: join(stage, 'app/relink'), to: 'relink', filter: ['**/*'] },
	];
	if (manifest.platform === 'linux') {
		resources.push({ from: icon('linux'), to: 'app-icon.png' });
	}
	return {
		appId: product.appId,
		productName: product.nameLong,
		electronVersion: manifest.electronVersion,
		electronDist: join(root, 'node_modules/electron/dist'),
		directories: { app: join(stage, 'app'), output, buildResources: join(stage, 'packaging-resources') },
		files: ['**/*', '!licenses/**', '!relink/**'],
		asar: true,
		asarUnpack: ['native/**', 'engine.cjs'],
		extraResources: resources,
		npmRebuild: false,
		nodeGypRebuild: false,
		publish: null,
		mac: {
			executableName: product.platforms.darwin.executableName,
			icon: icon('darwin'),
			minimumSystemVersion: product.platforms.darwin.minimumSystemVersion,
			helperBundleId: `${product.appId}.helper`,
			identity: null,
			hardenedRuntime: false,
			notarize: false,
			darkModeSupport: true,
			category: 'public.app-category.utilities',
			extendInfo: {
				NSCameraUsageDescription: 'Share your camera with computers you have paired and allowed.',
				NSMicrophoneUsageDescription: 'Share your microphone with computers you have paired and allowed.',
				NSAudioCaptureUsageDescription: 'Share this computer’s sound with computers you have paired and allowed.',
			},
		},
		win: { executableName: product.platforms.win32.executableName, icon: icon('win32'), signExecutable: false },
		linux: { executableName: product.platforms.linux.executableName, icon: icon('linux'), category: 'Network' },
	};
}

/** Never discover signing credentials or enable a Node/inspection launch mode during assembly. */
export function assemblyEnvironment(environment: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
	return Object.fromEntries(Object.entries({ ...environment, CSC_IDENTITY_AUTO_DISCOVERY: 'false' }).filter(([name]) => !/^(CSC_(?!IDENTITY_AUTO_DISCOVERY)|WIN_CSC_|APPLE_|ELECTRON_RUN_AS_NODE$|NODE_OPTIONS$)/.test(name)));
}

export function packagedLayout(directory: string, platform: NodeJS.Platform, product: Product): PackagedLayout {
	if (platform !== 'darwin' && platform !== 'win32' && platform !== 'linux') {
		throw new Error(`Unsupported packaging host: ${platform}`);
	}
	const app = platform === 'darwin' ? join(directory, `${product.nameLong}.app`, 'Contents') : directory;
	const resources = join(app, platform === 'darwin' ? 'Resources' : 'resources');
	return {
		executable: join(app, ...(platform === 'darwin' ? ['MacOS'] : []), `${product.platforms[platform].executableName}${platform === 'win32' ? '.exe' : ''}`),
		resources,
		asar: join(resources, 'app.asar'),
		native: join(resources, 'app.asar.unpacked/native/vnilink.node'),
	};
}

/** The electron-builder CLI flag for this host. */
export function builderPlatformFlag(): string {
	const flag = ({ darwin: '--mac', win32: '--win', linux: '--linux' } as Partial<Record<NodeJS.Platform, string>>)[process.platform];
	if (!flag) {
		throw new Error(`Unsupported packaging host: ${process.platform}`);
	}
	return flag;
}

async function readJson(path: string): Promise<{ version: string }> {
	return JSON.parse(await readFile(path, 'utf8')) as { version: string };
}

/** Assembles `dist/<platform>-<arch>` from the staged release; refuses to overlap another assembly. */
export async function packageApplication(root: string, signal: AbortSignal): Promise<string> {
	const stage = join(root, '.build/release', `${process.platform}-${process.arch}`);
	const manifest = await verifyReleaseFiles(stage);
	const product = validateProduct(JSON.parse(await readFile(join(stage, 'app/product.json'), 'utf8')));
	const electron = await readJson(join(root, 'node_modules/electron/package.json'));
	if (manifest.electronVersion !== electron.version) {
		throw new Error('Installed Electron differs from the staged release version');
	}
	const parent = join(root, 'dist');
	const output = join(parent, `${process.platform}-${process.arch}`);
	await mkdir(parent, { recursive: true });
	const lock = `${output}.lock`;
	await mkdir(lock);
	const processes = new ProcessRunner(signal);
	let work: string | undefined;
	try {
		work = await mkdtemp(join(parent, '.assemble-'));
		const buildOutput = join(work, 'output');
		const configPath = join(work, 'builder.json');
		await writeFile(configPath, JSON.stringify(packageConfiguration(root, stage, buildOutput, product, manifest), null, 2));
		const cli = createRequire(import.meta.url).resolve('electron-builder/cli.js');
		await processes.run(process.execPath, [cli, '--config', configPath, '--dir', builderPlatformFlag(), `--${process.arch}`, '--publish', 'never'], { cwd: root, env: assemblyEnvironment(process.env) });
		const candidates: string[] = [];
		for (const entry of await readdir(buildOutput, { withFileTypes: true })) {
			if (entry.isDirectory()) {
				const directory = join(buildOutput, entry.name);
				try {
					await access(packagedLayout(directory, process.platform, product).executable, constants.X_OK);
					candidates.push(directory);
				} catch (error) {
					if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
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
		const nativeSha256 = manifest.files.find(file => file.path === 'app/native/vnilink.node')?.sha256;
		if (!nativeSha256 || createHash('sha256').update(await readFile(layout.native)).digest('hex') !== nativeSha256) {
			throw new Error('Packager changed or omitted the staged native addon');
		}
		await access(join(layout.resources, 'licenses/native-codecs/native-codecs.txt'));
		await access(join(layout.resources, 'relink/source/Cargo.lock'));
		const packager = await readJson(join(root, 'node_modules/electron-builder/package.json'));
		const packaging = { appId: product.appId, version: manifest.version, electronVersion: electron.version, packagerVersion: packager.version, platform: process.platform, architecture: process.arch, signing: 'unsigned', executable: relative(directory, layout.executable).split('\\').join('/'), asarSha256: createHash('sha256').update(await readFile(layout.asar)).digest('hex'), nativeSha256, files: await packageInventory(directory) };
		await writeFile(join(directory, 'packaging-manifest.json'), JSON.stringify(packaging, null, 2) + '\n');
		await verifyReleaseFiles(stage);
		signal.throwIfAborted();
		await rm(output, { recursive: true, force: true });
		await rename(directory, output);
		console.log(`Unsigned Electron application assembled: ${output}`);
		return output;
	} finally {
		await processes.stop();
		if (work) {
			await rm(work, { recursive: true, force: true });
		}
		await rm(lock, { recursive: true, force: true });
	}
}
