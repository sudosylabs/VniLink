/** Build identity couples source inputs, target/profile and the staged napi binary. */
import { createHash } from 'node:crypto';
import { cp, readdir, readFile, writeFile } from 'node:fs/promises';
import { join, relative } from 'node:path';
import type { ProcessRunner } from './processes.ts';

export interface NativeOptions {
	/** Rust target triple; the host default when omitted. */
	readonly target?: string;
	readonly profile: 'dev' | 'release';
	/** Extra build environment, recorded in the identity. */
	readonly environment?: Readonly<Record<string, string>>;
}

export interface NativeIdentity {
	readonly source: string;
	readonly target: string | null;
	readonly profile: 'dev' | 'release';
	readonly platform: NodeJS.Platform;
	readonly architecture: string;
	readonly environment?: Readonly<Record<string, string>>;
}

/** The host's default addon target: Windows builds with the GNU toolchain. */
export function defaultNativeOptions(): NativeOptions {
	return { profile: 'dev', target: process.env.CARGO_BUILD_TARGET ?? (process.platform === 'win32' ? 'x86_64-pc-windows-gnu' : undefined) };
}

async function sourceFiles(root: string): Promise<string[]> {
	const files: string[] = [];
	const collect = async (directory: string): Promise<void> => {
		for (const entry of await readdir(directory, { withFileTypes: true })) {
			if (['target', '.git'].includes(entry.name)) {
				continue;
			}
			const path = join(directory, entry.name);
			if (entry.isDirectory()) {
				await collect(path);
			} else if (entry.isFile() && !path.endsWith('.md')) {
				files.push(path);
			}
		}
	};
	for (const directory of ['crates', 'vendor', '.cargo']) {
		await collect(join(root, directory));
	}
	files.push(...['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'].map(name => join(root, name)));
	return files.sort();
}

export async function nativeIdentity(root: string, options: NativeOptions): Promise<NativeIdentity> {
	const hash = createHash('sha256');
	for (const path of await sourceFiles(root)) {
		hash.update(relative(root, path));
		hash.update('\0');
		hash.update(await readFile(path));
		hash.update('\0');
	}
	const environment = options.environment && Object.keys(options.environment).length ? { environment: options.environment } : {};
	return { source: hash.digest('hex'), target: options.target ?? null, profile: options.profile, platform: process.platform, architecture: process.arch, ...environment };
}

function assertHostTarget(target: string | undefined): void {
	if (!target) {
		return;
	}
	const architecture = target.startsWith('aarch64-') ? 'arm64' : target.startsWith('x86_64-') ? 'x64' : undefined;
	const platform = target.includes('apple-darwin') ? 'darwin' : target.includes('windows') ? 'win32' : target.includes('linux') ? 'linux' : undefined;
	if (architecture !== process.arch || platform !== process.platform) {
		throw new Error('The staged Electron addon must target the current host platform and architecture');
	}
}

/** Builds the addon and returns the source identity it was built from. */
export async function compileNative(root: string, processes: Pick<ProcessRunner, 'run'>, options: NativeOptions): Promise<NativeIdentity> {
	assertHostTarget(options.target);
	const before = await nativeIdentity(root, options);
	const target = options.target ? ['--target', options.target] : [];
	const release = options.profile === 'release' ? ['--release'] : [];
	await processes.run('cargo', ['build', '--locked', '--target-dir', 'target/electron', ...target, ...release, '-p', 'vnilink-node'], { cwd: root, env: { ...process.env, ...options.environment } });
	return before;
}

/** Copies the addon built from `before` into `outputDirectory`, refusing sources changed since. */
export async function stageNative(root: string, outputDirectory: string, options: NativeOptions, before: NativeIdentity): Promise<void> {
	const identity = await nativeIdentity(root, options);
	if (JSON.stringify(identity) !== JSON.stringify(before)) {
		throw new Error('Native sources changed during build; rebuild before launching');
	}
	const filename = process.platform === 'win32' ? 'vnilink_node.dll' : process.platform === 'darwin' ? 'libvnilink_node.dylib' : 'libvnilink_node.so';
	const source = join(root, 'target/electron', options.target ?? '', options.profile === 'release' ? 'release' : 'debug', filename);
	const output = join(outputDirectory, 'native/vnilink.node');
	await cp(source, output);
	const binary = createHash('sha256').update(await readFile(output)).digest('hex');
	await writeFile(join(outputDirectory, 'native/build.json'), JSON.stringify({ ...identity, binary }) + '\n');
}

/** Throws unless the staged addon matches the current sources and its recorded binary. */
export async function assertNativeFresh(root: string, outputDirectory = join(root, 'out'), options = defaultNativeOptions()): Promise<void> {
	try {
		const saved: unknown = JSON.parse(await readFile(join(outputDirectory, 'native/build.json'), 'utf8'));
		const identity = await nativeIdentity(root, options);
		const binary = createHash('sha256').update(await readFile(join(outputDirectory, 'native/vnilink.node'))).digest('hex');
		if (JSON.stringify(saved) !== JSON.stringify({ ...identity, binary })) {
			throw new Error('Native build identity differs');
		}
	} catch (error) {
		throw new Error('Native addon missing or stale. Run npm run build. Rust is not watched.', { cause: error });
	}
}
