/**
 * The Electron that development, start and UI-test runs launch. The operating
 * system names and decorates a process from its executable: the macOS bundle,
 * the Windows file resources, the Linux file name and window icon. Like VS
 * Code's `.build/electron`, the stock distribution is copied once with the
 * product's name and artwork, in the layout of the packaged application.
 */
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { cp, mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import { ProcessRunner } from './processes.ts';
import { loadProduct, type Product } from './product.ts';
import { fileSha256, releaseInventory } from './verify.ts';

const require = createRequire(import.meta.url);

const run = promisify(execFile);

interface Branding {
	/** The distribution directory or bundle copied from the `electron` package. */
	readonly source: string;
	/** Where `source` lands inside the output directory. */
	readonly target: string;
	/** The executable, relative to the output directory. */
	readonly executable: string;
	brand(output: string, work: string): Promise<void>;
}

export interface ElectronOptions {
	/** Whether it will run with Chromium's sandbox; the Linux test container turns it off. */
	readonly sandboxed?: boolean;
}

/**
 * Returns the branded Electron executable, preparing it when it is missing or
 * stale. One that will be sandboxed is first checked to be allowed its sandbox.
 */
export async function electronExecutable(root: string, signal: AbortSignal, options: ElectronOptions = {}): Promise<string> {
	const executable = await preparedElectron(root, signal);
	if (process.platform === 'linux' && options.sandboxed !== false) {
		await assertLinuxSandbox(executable, signal);
	}
	return executable;
}

async function preparedElectron(root: string, signal: AbortSignal): Promise<string> {
	const product = await loadProduct(root);
	const branding = platformBranding(root, product, signal);
	const output = join(root, '.build/electron');
	const executable = join(output, branding.executable);
	const stamp = join(output, 'stamp.json');
	const fingerprint = JSON.stringify(await brandingFingerprint(root, product), null, 2) + '\n';
	if (await current(stamp, executable, fingerprint)) {
		return executable;
	}
	await mkdir(join(root, '.build'), { recursive: true });
	const stage = await mkdtemp(join(root, '.build/electron-'));
	try {
		const staged = join(stage, 'output');
		await cp(branding.source, join(staged, branding.target), { recursive: true, verbatimSymlinks: true, mode: constants.COPYFILE_FICLONE });
		await branding.brand(staged, stage);
		signal.throwIfAborted();
		await writeFile(join(staged, 'stamp.json'), fingerprint);
		await rm(output, { recursive: true, force: true });
		await rename(staged, output);
		console.log(`[electron] Prepared ${executable}`);
		return executable;
	} finally {
		await rm(stage, { recursive: true, force: true });
	}
}

function platformBranding(root: string, product: Product, signal: AbortSignal): Branding {
	// The `electron` package's main export is its executable.
	const stock = require('electron') as string;
	if (process.platform === 'darwin') {
		const bundle = `${product.nameLong}.app`;
		return {
			source: join(stock, '../../..'),
			target: bundle,
			executable: join(bundle, 'Contents/MacOS/Electron'),
			brand: (output, work) => brandDarwin(root, product, join(output, bundle), work, signal),
		};
	}
	if (process.platform === 'win32') {
		const executable = `${product.platforms.win32.executableName}.exe`;
		return { source: dirname(stock), target: '.', executable, brand: output => brandWin32(root, product, output, executable) };
	}
	if (process.platform === 'linux') {
		const executable = product.platforms.linux.executableName;
		return { source: dirname(stock), target: '.', executable, brand: output => brandLinux(root, product, output, executable) };
	}
	throw new Error(`No Electron branding for ${process.platform}`);
}

async function brandingFingerprint(root: string, product: Product): Promise<Record<string, string>> {
	const electron = (JSON.parse(await readFile(require.resolve('electron/package.json'), 'utf8')) as { version: string }).version;
	const icons: Record<string, string> = {};
	for (const platform of Object.values(product.platforms)) {
		const path = join(root, platform.icon);
		icons[platform.icon] = (await stat(path)).isDirectory() ? JSON.stringify(await releaseInventory(path)) : await fileSha256(path);
	}
	return { electron, platform: `${process.platform}-${process.arch}`, product: JSON.stringify(product), icons: JSON.stringify(icons) };
}

async function current(stamp: string, executable: string, fingerprint: string): Promise<boolean> {
	try {
		await stat(executable);
		return await readFile(stamp, 'utf8') === fingerprint;
	} catch {
		return false;
	}
}

/** macOS names the menu bar, Dock and permission prompts after the bundle's Info.plist. */
async function brandDarwin(root: string, product: Product, bundle: string, work: string, signal: AbortSignal): Promise<void> {
	const resources = join(bundle, 'Contents/Resources');
	const plist = join(bundle, 'Contents/Info.plist');
	const iconName = await compileIcon(root, product, resources, work, signal);
	await rm(join(resources, 'electron.icns'), { force: true });
	const values: readonly [string, string][] = [
		['CFBundleName', product.nameLong],
		['CFBundleDisplayName', product.nameLong],
		// A development identifier keeps TCC grants and Launch Services apart from an installed release.
		['CFBundleIdentifier', `${product.appId}.dev`],
		['CFBundleIconFile', iconName],
		['CFBundleIconName', iconName],
	];
	for (const [key, value] of values) {
		await run('plutil', ['-replace', key, '-string', value, plist], { signal });
	}
	// Editing Info.plist invalidates the outer ad-hoc seal; nested helpers keep their own signatures.
	await run('codesign', ['--force', '--sign', '-', bundle], { signal });
}

/** Compiles the Icon Composer asset with `actool`; without Xcode, falls back to the flat artwork. */
async function compileIcon(root: string, product: Product, resources: string, work: string, signal: AbortSignal): Promise<string> {
	const { icon, minimumSystemVersion, fallbackIconSource } = product.platforms.darwin;
	const name = 'AppIcon';
	try {
		await run('xcrun', ['actool', join(root, icon), '--compile', resources, '--app-icon', name, '--include-all-app-icons', '--output-partial-info-plist', join(work, 'icon.plist'), '--platform', 'macosx', '--target-device', 'mac', '--minimum-deployment-target', minimumSystemVersion, '--enable-on-demand-resources', 'NO', '--development-region', 'en', '--output-format', 'human-readable-text'], { signal });
		await stat(join(resources, `${name}.icns`));
		return name;
	} catch (error) {
		signal.throwIfAborted();
		console.warn(`[electron] actool could not compile ${icon}; using ${fallbackIconSource}.`, error instanceof Error ? error.message : error);
	}
	const iconset = join(work, `${name}.iconset`);
	await mkdir(iconset);
	for (const size of [16, 32, 128, 256, 512]) {
		for (const scale of [1, 2]) {
			const suffix = scale === 2 ? '@2x' : '';
			await run('sips', ['-z', String(size * scale), String(size * scale), join(root, fallbackIconSource), '--out', join(iconset, `icon_${String(size)}x${String(size)}${suffix}.png`)], { signal });
		}
	}
	await run('iconutil', ['-c', 'icns', iconset, '-o', join(resources, `${name}.icns`)], { signal });
	return name;
}

/** Windows takes the taskbar, Task Manager and permission names and icon from the executable's resources. */
async function brandWin32(root: string, product: Product, output: string, executable: string): Promise<void> {
	await rename(join(output, 'electron.exe'), join(output, executable));
	// Loaded only here: rcedit runs a Windows binary.
	const { rcedit } = await import('rcedit');
	await rcedit(join(output, executable), {
		icon: join(root, product.platforms.win32.icon),
		'version-string': {
			CompanyName: product.publisherName,
			FileDescription: product.nameLong,
			InternalFilename: product.platforms.win32.executableName,
			OriginalFilename: executable,
			ProductName: product.nameLong,
		},
	});
}

/**
 * Where AppArmor restricts unprivileged user namespaces (Ubuntu 23.10 and
 * later), Chromium's sandbox has them only through a profile that grants
 * them to its executable, as Ubuntu's own `code` and `chrome` profiles do,
 * or else needs the setuid `chrome-sandbox` helper; with neither, every
 * launch aborts. The helper's root ownership is lost whenever
 * `.build/electron` is prepared again, so the development Electron is
 * allowed by a profile, which `sandbox:linux` installs for its path.
 */
async function assertLinuxSandbox(executable: string, signal: AbortSignal): Promise<void> {
	const restricted = await readFile('/proc/sys/kernel/apparmor_restrict_unprivileged_userns', 'utf8').catch(() => '0');
	if (restricted.trim() !== '1') {
		return;
	}
	const helper = await stat(join(dirname(executable), 'chrome-sandbox')).catch(() => undefined);
	if (helper?.uid === 0 && (helper.mode & 0o4755) === 0o4755) {
		return;
	}
	// A profile attaches by path when the executable starts: as Node it reads
	// its own label, `unconfined` when none did.
	const { stdout } = await run(executable, ['-e', `process.stdout.write(require('node:fs').readFileSync('/proc/self/attr/current', 'utf8'))`], { env: { ...process.env, ELECTRON_RUN_AS_NODE: '1' }, signal });
	if (!stdout.trim().startsWith('unconfined')) {
		return;
	}
	throw new Error(`Electron's sandbox cannot start: AppArmor restricts unprivileged user namespaces here, and no profile allows them for ${executable}. Run \`node build/run.ts sandbox:linux\` once; it asks for sudo to install that profile.`);
}

/**
 * Installs the AppArmor profile that lets this checkout's development
 * Electron use Chromium's sandbox, named after the checkout so several can
 * be allowed. It grants user namespaces and nothing else is confined, as
 * Ubuntu's profile for Visual Studio Code does; it holds across preparing
 * `.build/electron` again because it names the path.
 */
export async function allowLinuxSandbox(root: string, signal: AbortSignal): Promise<void> {
	if (process.platform !== 'linux') {
		throw new Error('sandbox:linux allows the development Electron on Linux');
	}
	const product = await loadProduct(root);
	const executable = join(root, '.build/electron', product.platforms.linux.executableName);
	if (/["\n]/.test(executable)) {
		throw new Error(`An AppArmor profile cannot name ${JSON.stringify(executable)}`);
	}
	const name = `vnilink-development-${createHash('sha256').update(root).digest('hex').slice(0, 12)}`;
	const profile = [
		`# The VniLink development Electron in ${root}: user namespaces for`,
		'# Chromium\'s sandbox, and nothing confined, as Ubuntu\'s profile for Visual',
		'# Studio Code. Written by `node build/run.ts sandbox:linux`.',
		'abi <abi/4.0>,',
		'include <tunables/global>',
		'',
		`profile ${name} "${executable}" flags=(unconfined) {`,
		'  userns,',
		'}',
		'',
	].join('\n');
	const work = await mkdtemp(join(tmpdir(), 'vnilink-apparmor-'));
	const processes = new ProcessRunner(signal);
	try {
		const source = join(work, name);
		await writeFile(source, profile);
		const installed = `/etc/apparmor.d/${name}`;
		await processes.run('sudo', ['install', '-m', '0644', source, installed]);
		await processes.run('sudo', ['apparmor_parser', '--replace', installed]);
		console.log(`[electron] ${installed} allows the sandbox for ${executable}`);
	} finally {
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}

/** Linux window managers show the executable name and the window's icon, read from the packaged path. */
async function brandLinux(root: string, product: Product, output: string, executable: string): Promise<void> {
	await rename(join(output, 'electron'), join(output, executable));
	await cp(join(root, product.platforms.linux.icon), join(output, 'resources/app-icon.png'));
}
