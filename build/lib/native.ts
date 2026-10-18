/** Build identity couples source inputs, target/profile and the staged napi binary. */
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { cp, readdir, readFile, writeFile } from 'node:fs/promises';
import { join, relative, win32 } from 'node:path';
import type { ProcessRunner } from './processes.ts';

export interface NativeOptions {
	/** Rust target triple; the host default when omitted. */
	readonly target?: string;
	readonly profile: 'dev' | 'release';
	/** The oldest macOS the addon supports; requires an explicit Apple `target`. */
	readonly minimumSystemVersion?: string;
}

export interface NativeIdentity {
	readonly source: string;
	readonly target: string | null;
	readonly profile: 'dev' | 'release';
	readonly platform: NodeJS.Platform;
	readonly architecture: string;
	readonly minimumSystemVersion?: string;
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
	const minimum = options.minimumSystemVersion ? { minimumSystemVersion: options.minimumSystemVersion } : {};
	return { source: hash.digest('hex'), target: options.target ?? null, profile: options.profile, platform: process.platform, architecture: process.arch, ...minimum };
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

/** MSYS2's default install location; runners and custom installs put MinGW on PATH or name their root. */
const defaultMsysRoot = 'C:\\msys64';

/**
 * MinGW tools for the GNU Windows target: Rust links with them and calls
 * `dlltool`, and the native libraries build their C with them. A toolchain
 * already on PATH is used as it is (CI, a custom install, a prepared shell);
 * otherwise `VNILINK_MSYS_ROOT`, else MSYS2's default root, is added. Variables
 * the caller already set are kept.
 */
export function windowsGnuEnvironment(environment: NodeJS.ProcessEnv, exists: (path: string) => boolean = existsSync): NodeJS.ProcessEnv {
	const result: NodeJS.ProcessEnv = { ...environment };
	// Windows names it Path; a second PATH key would leave which one wins to the spawn.
	const pathKey = Object.keys(result).find(key => key.toUpperCase() === 'PATH') ?? 'PATH';
	const entries = (result[pathKey] ?? '').split(';').filter(Boolean);
	const hasTools = (directory: string): boolean => exists(win32.join(directory, 'dlltool.exe'));
	let bin = entries.find(hasTools);
	if (!bin) {
		const root = environment.VNILINK_MSYS_ROOT ?? defaultMsysRoot;
		const candidate = win32.join(root, 'mingw64', 'bin');
		if (!hasTools(candidate)) {
			throw new Error(`The Windows build needs MinGW (dlltool, gcc) and found none on PATH or in ${candidate}. `
				+ 'Install MSYS2 with mingw-w64-x86_64-gcc, -clang, -nasm and -make, then put its mingw64\\bin on PATH or set VNILINK_MSYS_ROOT to its root.');
		}
		bin = candidate;
		result[pathKey] = [bin, ...entries].join(';');
	}
	result.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER ??= 'gcc';
	result.CMAKE_GENERATOR ??= 'MinGW Makefiles';
	if (exists(win32.join(bin, 'libclang.dll'))) {
		result.LIBCLANG_PATH ??= bin;
	}
	return result;
}

/**
 * Cargo's environment for `options`. The GNU Windows target gets its MinGW
 * tools. A macOS minimum applies only to the target triple:
 * `MACOSX_DEPLOYMENT_TARGET` would also reach host build scripts and proc
 * macros, which macOS 27's loader then refuses to open, so the addon's link and
 * its C/C++ objects get the minimum through target-scoped variables instead.
 */
export function nativeEnvironment(
	options: NativeOptions,
	environment: NodeJS.ProcessEnv = process.env,
	exists: (path: string) => boolean = existsSync,
): NodeJS.ProcessEnv {
	const windowsGnu = options.target?.endsWith('-windows-gnu') === true;
	const result: NodeJS.ProcessEnv = windowsGnu ? windowsGnuEnvironment(environment, exists) : { ...environment };
	const minimum = options.minimumSystemVersion;
	if (!minimum) {
		return result;
	}
	const target = options.target;
	if (!target?.endsWith('-apple-darwin')) {
		throw new Error('A macOS minimum version requires an explicit Apple target, so host build tools keep their own');
	}
	if (environment.RUSTFLAGS || environment.CARGO_ENCODED_RUSTFLAGS) {
		throw new Error('RUSTFLAGS would replace the target-scoped macOS minimum; unset it for release builds');
	}
	delete result.MACOSX_DEPLOYMENT_TARGET;
	const flag = `-mmacosx-version-min=${minimum}`;
	const triple = target.replace(/-/g, '_');
	const rustflags = `CARGO_TARGET_${triple.toUpperCase()}_RUSTFLAGS`;
	result[rustflags] = [environment[rustflags], `-C link-arg=${flag}`].filter(Boolean).join(' ');
	for (const name of [`CFLAGS_${triple}`, `CXXFLAGS_${triple}`]) {
		result[name] = [environment[name], flag].filter(Boolean).join(' ');
	}
	return result;
}

/** Builds the addon and returns the source identity it was built from. */
export async function compileNative(root: string, processes: Pick<ProcessRunner, 'run'>, options: NativeOptions): Promise<NativeIdentity> {
	assertHostTarget(options.target);
	const before = await nativeIdentity(root, options);
	const target = options.target ? ['--target', options.target] : [];
	const release = options.profile === 'release' ? ['--release'] : [];
	const env = nativeEnvironment(options);
	await processes.run('cargo', ['build', '--locked', '--target-dir', 'target/electron', ...target, ...release, '-p', 'vnilink-node'], { cwd: root, env });
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
