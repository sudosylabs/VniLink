/**
 * What each task needs on this computer, read from `build/prerequisites.json`,
 * and whether the computer has it. `npm run doctor` reports every task,
 * `preinstall` the ones a fresh clone runs first, and `build/run.ts` checks a
 * task's own requirements before running it.
 *
 * Imports only Node's own modules: `preinstall` must not depend on what it
 * checks, and the Linux image copies this file alone with the manifest.
 *
 * Run directly, `node build/lib/prerequisites.ts packages <debian|fedora|msys2>
 * <task>...` prints the packages those tasks need, for the container image
 * and CI.
 */
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access, readdir, readFile, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { posix, win32 } from 'node:path';
import { fileURLToPath } from 'node:url';

export type Platform = 'darwin' | 'win32' | 'linux';

export type Distribution = 'debian' | 'fedora';

export type Status = 'ready' | 'missing' | 'unsupported' | 'unknown';

/**
 * What a requirement is: something to install (the default), a permission
 * the OS grants a process, or another computer a task can use.
 */
export type RequirementKind = 'tool' | 'permission' | 'computer';

/** Where one command, library or pkg-config module comes from on each OS. */
interface Source {
	readonly debian?: string;
	readonly fedora?: string;
	readonly darwin?: string;
	readonly win32?: string;
	/** MSYS2 packages, for Windows. */
	readonly msys2?: string;
	/** What needs it, for the report. */
	readonly for?: string;
	/** Host architectures that need it; every one when omitted. */
	readonly architectures?: readonly string[];
}

interface Check {
	readonly custom?: string;
	readonly commands?: Readonly<Record<string, Source>>;
	readonly 'pkg-config'?: Readonly<Record<string, Source>>;
	readonly libraries?: Readonly<Record<string, Source>>;
	readonly packages?: Source;
	readonly tools?: readonly string[];
}

export interface Requirement {
	readonly title: string;
	readonly kind?: RequirementKind;
	readonly purpose?: string;
	readonly platforms?: readonly Platform[];
	readonly optional?: boolean;
	/** `npm ci` prepares it (postinstall, or an npm dependency). */
	readonly prepared?: boolean;
	readonly check: Check;
	readonly fix?: Readonly<Partial<Record<string, string>>>;
}

export interface TaskEntry {
	readonly description: string;
	/** The command when it is not `node build/run.ts <task>`. */
	readonly run?: string;
	readonly platforms?: readonly Platform[];
	readonly requires?: readonly string[];
	/** Another task whose requirements this one also has. */
	readonly same?: string;
}

interface Architecture {
	readonly status: 'supported' | 'unverified' | 'unsupported';
	readonly reason?: string;
}

interface DistributionEntry {
	readonly name: string;
	readonly ids: readonly string[];
	readonly status: string;
	/** Which versions have run the suite. */
	readonly verified: string;
	/** The install command, before the package names. */
	readonly install: string;
}

interface PlatformEntry {
	readonly name: string;
	readonly rustTarget?: string;
	readonly architectures: Readonly<Record<string, Architecture>>;
	readonly distributions?: Readonly<Record<Distribution, DistributionEntry>>;
	readonly desktops?: { readonly supported: readonly string[]; readonly reason: string };
}

export interface Download {
	readonly what: string;
	readonly into: string;
	readonly platforms: readonly Platform[];
	/** Where it is kept under the home directory; otherwise `into` is under the checkout. Either existing means it was done. */
	readonly cache?: string;
}

export interface Manifest {
	readonly node: { readonly versionFile: string; readonly install: string };
	readonly rust: { readonly toolchainFile: string; readonly install: string };
	readonly platforms: Readonly<Record<Platform, PlatformEntry>>;
	readonly requirements: Readonly<Record<string, Requirement>>;
	readonly tasks: Readonly<Record<string, TaskEntry>>;
	readonly downloads: readonly Download[];
}

/** The computer being checked. */
export interface Host {
	readonly root: string;
	readonly platform: NodeJS.Platform;
	/** Node's architecture. */
	readonly architecture: string;
	/** The processor's: an Apple silicon Mac running x64 Node under Rosetta reports `arm64` here. */
	readonly hardwareArchitecture: string;
	readonly nodeVersion: string;
	readonly osName: string;
	readonly distribution?: Distribution | undefined;
	readonly env: NodeJS.ProcessEnv;
	/** The process's user id where the OS has one; 0 is root. */
	readonly uid?: number | undefined;
}

export interface ProbeResult {
	readonly ok: boolean;
	readonly stdout: string;
}

/** Everything the checks ask the computer; tests replace it. */
export interface Probe {
	run(command: string, args: readonly string[], env?: NodeJS.ProcessEnv): Promise<ProbeResult>;
	exists(path: string): Promise<boolean>;
	readFile(path: string): Promise<string | undefined>;
	readdir(path: string): Promise<readonly string[]>;
	stat(path: string): Promise<{ readonly uid: number; readonly mode: number } | undefined>;
}

export interface CheckResult {
	readonly id: string;
	readonly title: string;
	readonly kind: RequirementKind;
	readonly status: Status;
	readonly optional: boolean;
	/** What was found, or what is missing. */
	readonly detail?: string;
	/** A warning that does not block. */
	readonly note?: string;
	/** Commands or steps that fix it. */
	readonly fix: readonly string[];
	/** Linux packages that fix it, on a known distribution. */
	readonly packages: readonly string[];
}

export interface TaskReport {
	readonly name: string;
	readonly description: string;
	/** `unknown`: nothing is missing, but a requirement cannot be checked before the task runs. */
	readonly status: 'ready' | 'missing' | 'unsupported' | 'unknown';
	readonly reason?: string;
	readonly results: readonly CheckResult[];
}

const manifestUrl = new URL('../prerequisites.json', import.meta.url);

export async function loadManifest(): Promise<Manifest> {
	return JSON.parse(await readFile(manifestUrl, 'utf8')) as Manifest;
}

export const systemProbe: Probe = {
	run: (command, args, env) => new Promise(resolve => {
		execFile(command, args, { env: env ?? process.env, timeout: 20_000, windowsHide: true }, (error, stdout) => {
			resolve({ ok: !error, stdout });
		});
	}),
	exists: path => access(path).then(() => true, () => false),
	readFile: path => readFile(path, 'utf8').catch(() => undefined),
	readdir: path => readdir(path).catch(() => []),
	stat: path => stat(path).then(({ uid, mode }) => ({ uid, mode }), () => undefined),
};

function osRelease(text: string): Partial<Record<string, string>> {
	const values: Partial<Record<string, string>> = {};
	for (const line of text.split('\n')) {
		const match = /^([A-Z_]+)=(.*)$/.exec(line.trim());
		if (match) {
			values[match[1]] = match[2].replace(/^"(.*)"$/, '$1');
		}
	}
	return values;
}

/** The distribution family whose package names apply, from `/etc/os-release`. */
export function distributionOf(manifest: Manifest, text: string): Distribution | undefined {
	const release = osRelease(text);
	const ids = [release.ID, ...(release.ID_LIKE ?? '').split(' ')].filter(Boolean);
	const distributions = manifest.platforms.linux.distributions;
	if (!distributions) {
		return undefined;
	}
	return (Object.keys(distributions) as Distribution[]).find(name => distributions[name].ids.some(id => ids.includes(id)));
}

export async function detectHost(root: string, manifest: Manifest, probe: Probe = systemProbe): Promise<Host> {
	const base = {
		root, platform: process.platform, architecture: process.arch, nodeVersion: process.versions.node, env: process.env, uid: process.getuid?.(),
	};
	if (process.platform === 'darwin') {
		// Rosetta hides the processor from Node; the kernel still reports it.
		// By path: a user's PATH may leave out sbin, and a failed query would read as Intel.
		const arm64 = (await probe.run('/usr/sbin/sysctl', ['-n', 'hw.optional.arm64'])).stdout.trim() === '1';
		const version = (await probe.run('/usr/bin/sw_vers', ['-productVersion'])).stdout.trim();
		return { ...base, hardwareArchitecture: arm64 ? 'arm64' : 'x64', osName: `macOS ${version}` };
	}
	if (process.platform === 'linux') {
		const text = await probe.readFile('/etc/os-release') ?? '';
		return { ...base, hardwareArchitecture: process.arch, osName: osRelease(text).PRETTY_NAME ?? 'Linux', distribution: distributionOf(manifest, text) };
	}
	return { ...base, hardwareArchitecture: process.arch, osName: process.platform === 'win32' ? 'Windows' : process.platform };
}

/** Paths as `platform` writes them: a host is described, not necessarily the computer running the check. */
function pathsOf(platform: NodeJS.Platform): typeof posix {
	return platform === 'win32' ? win32 : posix;
}

/** Finds `name` on PATH without starting a process. */
async function findCommand(host: Host, probe: Probe, name: string): Promise<string | undefined> {
	const paths = pathsOf(host.platform);
	const pathKey = Object.keys(host.env).find(key => key.toUpperCase() === 'PATH') ?? 'PATH';
	const directories = (host.env[pathKey] ?? '').split(paths.delimiter).filter(Boolean);
	// Administration tools live in sbin, which a user's PATH may leave out.
	if (host.platform === 'linux') {
		directories.push('/usr/sbin', '/sbin');
	}
	const extensions = host.platform === 'win32' && !/\.[a-z]+$/i.test(name)
		? (host.env.PATHEXT ?? '.COM;.EXE;.BAT;.CMD').split(';').filter(Boolean)
		: [''];
	for (const directory of directories) {
		for (const extension of extensions) {
			const path = paths.join(directory, name + extension);
			if (await probe.exists(path)) {
				return path;
			}
		}
	}
	return undefined;
}

function applies(source: Source, host: Host): boolean {
	return !source.architectures || source.architectures.includes(host.hardwareArchitecture);
}

/** How to get `source` here: a Linux package name, or an instruction for macOS and Windows. */
function sourceFix(source: Source, host: Host): { readonly package?: string; readonly step?: string } {
	if (host.platform === 'linux') {
		const name = host.distribution ? source[host.distribution] : undefined;
		if (name) {
			return { package: name };
		}
		return { step: `the package providing it (Debian: ${source.debian ?? '?'}, Fedora: ${source.fedora ?? '?'})` };
	}
	const step = host.platform === 'darwin' ? source.darwin : host.platform === 'win32' ? source.win32 : undefined;
	return step ? { step } : {};
}

/** The requirement's own fix for `key`, the distribution, the OS, or its default. */
function requirementFix(requirement: Requirement, host: Host, key?: string): string[] {
	const fix = requirement.fix ?? {};
	const text = (key ? fix[key] : undefined) ?? (host.distribution ? fix[host.distribution] : undefined) ?? fix[host.platform] ?? fix.default;
	return text ? [text.replace('{msys2}', requirement.check.packages?.msys2 ?? '')] : [];
}

export interface Outcome {
	readonly status: Status;
	readonly detail?: string;
	readonly note?: string;
	readonly fix?: readonly string[];
	readonly packages?: readonly string[];
	/** Selects `fix[key]` of the requirement. */
	readonly fixKey?: string;
}

/** Checks a table of named things, reporting the missing ones with where to get them. */
async function checkTable(
	host: Host,
	entries: Readonly<Record<string, Source>>,
	present: (name: string) => Promise<boolean>,
	describe: (missing: readonly string[]) => string,
): Promise<Outcome> {
	const missing: string[] = [];
	const packages = new Set<string>();
	const steps = new Set<string>();
	for (const [name, source] of Object.entries(entries)) {
		if (!applies(source, host) || await present(name)) {
			continue;
		}
		missing.push(name);
		const fix = sourceFix(source, host);
		if (fix.package) {
			packages.add(fix.package);
		}
		if (fix.step) {
			steps.add(host.platform === 'linux' ? `${name}: ${fix.step}` : fix.step);
		}
	}
	if (!missing.length) {
		return { status: 'ready' };
	}
	return { status: 'missing', detail: describe(missing), fix: [...steps], packages: [...packages] };
}

function nodeRequirement(text: string): readonly [number, number, number] | undefined {
	const match = /^v?(\d+)\.(\d+)\.(\d+)/.exec(text.trim());
	return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : undefined;
}

/** VS Code's rule: the major version of `.nvmrc`, at least its minor and patch. */
export function nodeVersionAccepted(version: string, required: string): boolean {
	const have = nodeRequirement(version);
	const need = nodeRequirement(required);
	if (!have || !need) {
		return false;
	}
	return have[0] === need[0] && (have[1] > need[1] || (have[1] === need[1] && have[2] >= need[2]));
}

/** MSYS2's default install location; runners and custom installs put MinGW on PATH or name their root. */
const defaultMsysRoot = 'C:\\msys64';

/** Where MinGW may be, in order: each PATH entry, then `VNILINK_MSYS_ROOT` (else MSYS2's default root). */
export function mingwCandidates(environment: NodeJS.ProcessEnv): string[] {
	const pathKey = Object.keys(environment).find(key => key.toUpperCase() === 'PATH') ?? 'PATH';
	const entries = (environment[pathKey] ?? '').split(';').filter(Boolean);
	return [...entries, win32.join(environment.VNILINK_MSYS_ROOT ?? defaultMsysRoot, 'mingw64', 'bin')];
}

/** The first candidate directory with MinGW's `dlltool`, which Rust's GNU target calls. */
export async function mingwBin(environment: NodeJS.ProcessEnv, exists: (path: string) => boolean | Promise<boolean>): Promise<string | undefined> {
	for (const directory of mingwCandidates(environment)) {
		if (await exists(win32.join(directory, 'dlltool.exe'))) {
			return directory;
		}
	}
	return undefined;
}

/** The macOS Liquid Glass prebuild of `architecture`, from the optional @vnilink/elg dependency. */
export function glassPrebuild(root: string, architecture: string): string {
	return posix.join(root, 'node_modules/@vnilink/elg/prebuilds', `darwin-${architecture}`, 'node.napi.node');
}

/** The channel `rust-toolchain.toml` selects. */
export function rustChannel(text: string): string | undefined {
	return /^\s*channel\s*=\s*"([^"]+)"/m.exec(text)?.[1];
}

/** The AppArmor profile `sandbox:linux` installs for the checkout at `root`. */
export function linuxSandboxProfile(root: string): string {
	return `vnilink-development-${createHash('sha256').update(root).digest('hex').slice(0, 12)}`;
}

async function linuxExecutable(root: string, probe: Probe): Promise<string> {
	const product = JSON.parse(await probe.readFile(posix.join(root, 'product.json')) ?? '{}') as { platforms?: { linux?: { executableName?: string } } };
	return posix.join(root, '.build/electron', product.platforms?.linux?.executableName ?? 'vnilink');
}

/**
 * Whether the development Electron may use Chromium's sandbox. Where AppArmor
 * restricts unprivileged user namespaces (Ubuntu 23.10 and later), the sandbox
 * has them only through a profile naming the executable, as Ubuntu's `code`
 * and `chrome` profiles do, or through a setuid-root `chrome-sandbox`; with
 * neither, every launch aborts.
 */
export async function linuxSandbox(root: string, probe: Probe, env: NodeJS.ProcessEnv = process.env): Promise<Outcome> {
	if (env.VNILINK_TEST_CONTAINER === '1') {
		// Docker's seccomp policy blocks the namespaces anyway; the test image runs Electron without its sandbox.
		return { status: 'ready', detail: 'the test container runs Electron without its sandbox' };
	}
	const restricted = await probe.readFile('/proc/sys/kernel/apparmor_restrict_unprivileged_userns');
	if (restricted?.trim() !== '1') {
		return { status: 'ready', detail: 'user namespaces are not restricted' };
	}
	const executable = await linuxExecutable(root, probe);
	const helper = await probe.stat(posix.join(executable, '..', 'chrome-sandbox'));
	if (helper?.uid === 0 && (helper.mode & 0o4755) === 0o4755) {
		return { status: 'ready', detail: 'chrome-sandbox is setuid root' };
	}
	if (await probe.exists(executable)) {
		// A profile attaches by path when the executable starts: as Node it
		// reads its own label, `unconfined` when none did.
		const label = await probe.run(executable, ['-e', `process.stdout.write(require('node:fs').readFileSync('/proc/self/attr/current', 'utf8'))`], { ...env, ELECTRON_RUN_AS_NODE: '1' });
		if (label.ok && !label.stdout.trim().startsWith('unconfined')) {
			return { status: 'ready', detail: `AppArmor profile ${label.stdout.trim()}` };
		}
	} else if (await probe.exists(`/etc/apparmor.d/${linuxSandboxProfile(root)}`)) {
		return { status: 'ready', detail: `/etc/apparmor.d/${linuxSandboxProfile(root)}` };
	}
	return { status: 'missing', detail: `AppArmor restricts user namespaces and no profile allows them for ${executable}` };
}

async function hasLibclang(host: Host, probe: Probe): Promise<boolean> {
	const directories = [host.env.LIBCLANG_PATH, '/usr/lib', '/usr/lib64', '/usr/local/lib', `/usr/lib/${host.hardwareArchitecture === 'arm64' ? 'aarch64' : 'x86_64'}-linux-gnu`];
	for (const entry of await probe.readdir('/usr/lib')) {
		if (/^llvm(-\d+)?$/.test(entry)) {
			directories.push(`/usr/lib/${entry}/lib`);
		}
	}
	for (const directory of directories) {
		if (directory && (await probe.readdir(directory)).some(name => /^libclang(-[\d.]+)?\.so(\.\d+)*$/.test(name))) {
			return true;
		}
	}
	return false;
}

async function libraries(probe: Probe): Promise<Set<string> | undefined> {
	for (const ldconfig of ['/sbin/ldconfig', '/usr/sbin/ldconfig', 'ldconfig']) {
		const result = await probe.run(ldconfig, ['-p']);
		if (result.ok) {
			return new Set(result.stdout.split('\n').map(line => line.trim().split(' ')[0]).filter(Boolean));
		}
	}
	return undefined;
}

/** Whether a session bus owns, or can start, the Secret Service. */
async function secretService(host: Host, probe: Probe): Promise<Outcome> {
	const busctl = await probe.run('busctl', ['--user', '--no-pager', '--no-legend', 'list']);
	if (busctl.ok) {
		return busctl.stdout.split('\n').some(line => line.split(/\s+/)[0] === 'org.freedesktop.secrets')
			? { status: 'ready', detail: 'org.freedesktop.secrets is on the session bus' }
			: { status: 'missing', detail: 'no Secret Service (org.freedesktop.secrets) on the session bus' };
	}
	if (!host.env.DBUS_SESSION_BUS_ADDRESS) {
		return { status: 'missing', detail: 'no D-Bus session bus' };
	}
	return { status: 'unknown', detail: 'busctl is unavailable to ask the session bus' };
}

/**
 * Whether macOS grants the application this task runs under a permission,
 * asked without prompting: the preflight functions only read the grant,
 * where their requesting counterparts would ask. macOS attributes a child
 * process to the application that started the chain (the terminal or
 * editor), so osascript's answer is the one a driver started by the task
 * gets. Undefined when osascript does not answer.
 */
async function macPermission(probe: Probe, framework: string, preflight: string): Promise<boolean | undefined> {
	const script = `ObjC.import('${framework}'); ObjC.bindFunction('${preflight}', ['bool', []]); $.${preflight}()`;
	const result = await probe.run('/usr/bin/osascript', ['-l', 'JavaScript', '-e', script]);
	const answer = result.stdout.trim();
	return result.ok && (answer === 'true' || answer === 'false') ? answer === 'true' : undefined;
}

function macPermissionOutcome(granted: boolean | undefined): Outcome {
	if (granted === undefined) {
		return { status: 'unknown', detail: 'macOS did not answer whether the application running this task has it' };
	}
	return granted
		? { status: 'ready', detail: 'granted to the application running this task' }
		: { status: 'missing', detail: 'not granted to the application running this task (the terminal or editor that started it)' };
}

/** Files in which `measure:latency` keeps the desktop portal's answers, host's and reader's. */
export function latencyPortalGrants(root: string): string[] {
	return ['host-screen-grant', 'reader-screen-grant'].map(name => posix.join(root, '.build/latency', name));
}

type CustomCheck = (requirement: Requirement, manifest: Manifest, host: Host, probe: Probe) => Promise<Outcome>;

const customChecks: Readonly<Record<string, CustomCheck>> = {
	platform: (_requirement, manifest, host) => {
		const entry = Object.hasOwn(manifest.platforms, host.platform) ? manifest.platforms[host.platform as Platform] : undefined;
		if (!entry) {
			return Promise.resolve({ status: 'unsupported', detail: `${host.platform} is not supported` });
		}
		const architecture = Object.hasOwn(entry.architectures, host.hardwareArchitecture) ? entry.architectures[host.hardwareArchitecture] : undefined;
		const name = `${host.osName} ${host.hardwareArchitecture}`;
		if (!architecture || architecture.status === 'unsupported') {
			return Promise.resolve({ status: 'unsupported', detail: architecture?.reason ?? `${entry.name} on ${host.hardwareArchitecture} is not supported` });
		}
		const notes: string[] = [];
		if (architecture.status === 'unverified' && architecture.reason) {
			notes.push(architecture.reason);
		}
		if (host.platform === 'linux') {
			if (!host.distribution) {
				notes.push('This distribution is unverified; the package names shown are Ubuntu/Debian and Fedora ones.');
			}
			const desktop = host.env.XDG_CURRENT_DESKTOP;
			if (desktop && entry.desktops && !entry.desktops.supported.some(name => desktop.split(':').includes(name))) {
				notes.push(`${desktop}: ${entry.desktops.reason}`);
			}
		}
		return Promise.resolve({ status: 'ready', detail: name, ...notes.length ? { note: notes.join(' ') } : {} });
	},
	node: async (_requirement, manifest, host, probe) => {
		const required = (await probe.readFile(pathsOf(host.platform).join(host.root, manifest.node.versionFile)))?.trim() ?? '';
		if (!nodeVersionAccepted(host.nodeVersion, required)) {
			const major = nodeRequirement(required)?.[0] ?? '?';
			return { status: 'missing', detail: `Node ${host.nodeVersion}; needs ${required} or a later ${String(major)}.x (${manifest.node.versionFile})`, fix: [manifest.node.install] };
		}
		if (host.architecture !== host.hardwareArchitecture) {
			return { status: 'missing', detail: `Node is ${host.architecture} on an ${host.hardwareArchitecture} computer`, fix: [`Install the ${host.hardwareArchitecture} build of Node ${required}.`] };
		}
		return { status: 'ready', detail: `${host.nodeVersion} ${host.architecture}` };
	},
	rust: async (_requirement, manifest, host, probe) => {
		const channel = rustChannel(await probe.readFile(pathsOf(host.platform).join(host.root, manifest.rust.toolchainFile)) ?? '');
		if (!await findCommand(host, probe, 'rustup')) {
			return { status: 'missing', detail: 'rustup is not installed', fixKey: 'missing-rustup' };
		}
		const toolchains = await probe.run('rustup', ['toolchain', 'list']);
		if (!channel || toolchains.stdout.split('\n').some(line => line.startsWith(`${channel}-`))) {
			return { status: 'ready', detail: channel ?? 'rustup' };
		}
		return { status: 'missing', detail: `Rust ${channel} (${manifest.rust.toolchainFile}) is not installed` };
	},
	'rust-target': async (_requirement, manifest, host, probe) => {
		const target = manifest.platforms.win32.rustTarget ?? '';
		const channel = rustChannel(await probe.readFile(pathsOf(host.platform).join(host.root, manifest.rust.toolchainFile)) ?? '') ?? 'stable';
		const installed = await probe.run('rustup', ['target', 'list', '--installed', '--toolchain', channel]);
		return installed.stdout.split('\n').map(line => line.trim()).includes(target)
			? { status: 'ready', detail: target }
			: { status: 'missing', detail: `${target} is not installed for Rust ${channel}` };
	},
	'rust-windows-host': async (_requirement, _manifest, host, probe) => {
		const shown = await probe.run('rustup', ['show']);
		const triple = /Default host:\s*(\S+)/.exec(shown.stdout)?.[1];
		if (!triple) {
			return { status: 'unknown', detail: 'rustup did not report its host' };
		}
		if (triple.endsWith('-gnu')) {
			return { status: 'ready', detail: triple };
		}
		const programs = host.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)';
		const vswhere = win32.join(programs, 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
		const tools = await probe.run(vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']);
		return tools.ok && tools.stdout.trim()
			? { status: 'ready', detail: `${triple}; build scripts link with Visual Studio's tools` }
			: { status: 'missing', detail: `the host is ${triple}, whose build scripts need Visual Studio's linker` };
	},
	'xcode-tools': async (_requirement, _manifest, _host, probe) => {
		const developer = await probe.run('xcode-select', ['-p']);
		const clang = developer.ok ? await probe.run('xcrun', ['--find', 'clang']) : developer;
		return clang.ok ? { status: 'ready', detail: developer.stdout.trim() } : { status: 'missing', detail: 'no developer tools are selected' };
	},
	xcode: async (_requirement, _manifest, _host, probe) => {
		const actool = await probe.run('xcrun', ['--find', 'actool']);
		return actool.ok ? { status: 'ready', detail: actool.stdout.trim() } : { status: 'missing', detail: 'actool is unavailable; the development app uses the flat icon' };
	},
	'liquid-glass': async (_requirement, _manifest, host, probe) => {
		const path = glassPrebuild(host.root, host.architecture);
		return await probe.exists(path) ? { status: 'ready' } : { status: 'missing', detail: `${path} is missing` };
	},
	mingw: async (requirement, _manifest, host, probe) => {
		const found = await mingwBin(host.env, path => probe.exists(path));
		if (!found) {
			return { status: 'missing', detail: `no MinGW on PATH or in ${host.env.VNILINK_MSYS_ROOT ?? 'C:\\msys64'}` };
		}
		const missing: string[] = [];
		for (const tool of requirement.check.tools ?? []) {
			if (!await probe.exists(win32.join(found, tool))) {
				missing.push(tool);
			}
		}
		return missing.length ? { status: 'missing', detail: `${found} lacks ${missing.join(', ')}` } : { status: 'ready', detail: found };
	},
	libclang: async (requirement, _manifest, host, probe) => {
		if (await hasLibclang(host, probe)) {
			return { status: 'ready' };
		}
		const fix = requirement.check.packages ? sourceFix(requirement.check.packages, host) : {};
		return { status: 'missing', detail: 'no libclang shared library', packages: fix.package ? [fix.package] : [], fix: fix.step ? [`libclang: ${fix.step}`] : [] };
	},
	'desktop-session': (_requirement, _manifest, host) => {
		const display = host.env.WAYLAND_DISPLAY ? `WAYLAND_DISPLAY=${host.env.WAYLAND_DISPLAY}` : host.env.DISPLAY ? `DISPLAY=${host.env.DISPLAY}` : undefined;
		return Promise.resolve(display ? { status: 'ready', detail: display } : { status: 'missing', detail: 'neither DISPLAY nor WAYLAND_DISPLAY is set' });
	},
	'secret-service': async (requirement, _manifest, host, probe) => {
		const outcome = await secretService(host, probe);
		if (outcome.status !== 'missing') {
			return outcome;
		}
		const fix = requirement.check.packages ? sourceFix(requirement.check.packages, host) : {};
		return { ...outcome, packages: fix.package ? [fix.package] : [] };
	},
	'test-display': async (requirement, _manifest, host, probe) => {
		const display = host.env.WAYLAND_DISPLAY ?? host.env.DISPLAY;
		if (display) {
			return { status: 'ready', detail: `display ${display}` };
		}
		const tools = await checkTable(host, requirement.check.commands ?? {}, async name => Boolean(await findCommand(host, probe, name)), missing => `no display, and running headless needs ${missing.join(', ')}`);
		return tools.status === 'ready'
			? { status: 'ready', detail: 'no display', note: 'There is no display: run the UI tests with scripts/test.sh, which starts Xvfb, a window manager and a temporary keyring.' }
			: tools;
	},
	'linux-sandbox': (_requirement, _manifest, host, probe) => linuxSandbox(host.root, probe, host.env),
	apparmor: async (_requirement, _manifest, host, probe) => {
		if (!await probe.exists('/sys/kernel/security/apparmor')) {
			return { status: 'unsupported', detail: 'AppArmor is not active here, so Electron\'s sandbox needs no profile' };
		}
		return await findCommand(host, probe, 'apparmor_parser')
			? { status: 'ready' }
			: { status: 'missing', detail: 'apparmor_parser is missing', ...host.distribution === 'debian' ? { packages: ['apparmor'] } : {} };
	},
	'screen-recording': async (_requirement, _manifest, _host, probe) => macPermissionOutcome(await macPermission(probe, 'CoreGraphics', 'CGPreflightScreenCaptureAccess')),
	accessibility: async (_requirement, _manifest, _host, probe) => macPermissionOutcome(await macPermission(probe, 'ApplicationServices', 'AXIsProcessTrusted')),
	// macOS asks for the development application by its own identity, and
	// only that application can ask whether it holds the grant.
	'vnilink-screen-recording': () => Promise.resolve({ status: 'unknown', detail: 'macOS asks for the development VniLink when a run starts; nothing here can read its grant beforehand' }),
	'screen-portal': async (_requirement, _manifest, host, probe) => {
		const grants = latencyPortalGrants(host.root);
		for (const grant of grants) {
			if (!await probe.exists(grant)) {
				return { status: 'unknown', detail: 'the desktop portal asks someone at this computer when a run starts, until a run keeps its answer' };
			}
		}
		return { status: 'ready', detail: 'an earlier run kept the portal\'s answers in .build/latency' };
	},
	'signed-in-desktop': (_requirement, _manifest, host) => Promise.resolve(host.env.SESSIONNAME
		? { status: 'ready', detail: `session ${host.env.SESSIONNAME}` }
		: { status: 'missing', detail: 'this process is in no signed-in desktop (SESSIONNAME is unset, as over SSH or in a service)' }),
	root: (_requirement, _manifest, host) => Promise.resolve(host.uid === 0 ? { status: 'ready' } : { status: 'missing', detail: 'not running as root' }),
	// Nothing here can reach another computer before a run names one.
	'second-computer': () => Promise.resolve({ status: 'unknown', detail: '--peer runs a suite or a measurement\'s other side against `vnilink-testkit agent` there, built from the same commit, once a person types this computer\'s digits on its terminal; every task runs on this computer without it' }),
	docker: async (_requirement, _manifest, host, probe) => {
		if (!await findCommand(host, probe, 'docker')) {
			return { status: 'missing', detail: 'docker is not installed' };
		}
		const info = await probe.run('docker', ['info', '--format', '{{.OSType}}/{{.Architecture}}']);
		return info.ok ? { status: 'ready', detail: info.stdout.trim() } : { status: 'missing', detail: 'the Docker daemon is not running or not reachable' };
	},
};

export async function checkRequirement(id: string, manifest: Manifest, host: Host, probe: Probe): Promise<CheckResult> {
	const requirement = manifest.requirements[id];
	const { check } = requirement;
	const outcomes: Outcome[] = [];
	if (check.custom) {
		const custom = customChecks[check.custom];
		outcomes.push(await custom(requirement, manifest, host, probe));
	} else {
		if (check.commands) {
			outcomes.push(await checkTable(host, check.commands, async name => Boolean(await findCommand(host, probe, name)), missing => `${missing.join(', ')} not found`));
		}
		if (check['pkg-config']) {
			const modules = check['pkg-config'];
			const pkgConfig = await findCommand(host, probe, 'pkg-config');
			outcomes.push(await checkTable(host, modules, async name => Boolean(pkgConfig) && (await probe.run('pkg-config', ['--exists', name])).ok, missing => `pkg-config cannot find ${missing.join(', ')}`));
		}
		if (check.libraries) {
			const found = await libraries(probe);
			outcomes.push(found
				? await checkTable(host, check.libraries, name => Promise.resolve(found.has(name)), missing => `${missing.join(', ')} not found`)
				: { status: 'unknown', detail: 'ldconfig is unavailable to list shared libraries' });
		}
	}
	const failed = outcomes.filter(outcome => outcome.status !== 'ready');
	const status: Status = failed.find(outcome => outcome.status === 'unsupported')?.status
		?? failed.find(outcome => outcome.status === 'missing')?.status
		?? failed.at(0)?.status
		?? 'ready';
	const shown = failed.length ? failed : outcomes;
	const detail = shown.map(outcome => outcome.detail).filter(Boolean).join('; ');
	const note = outcomes.map(outcome => outcome.note).filter(Boolean).join(' ');
	const packages = [...new Set(failed.flatMap(outcome => outcome.packages ?? []))];
	// A requirement's own fix applies when its check named no package or step.
	const steps = failed.flatMap(outcome => outcome.fix ?? []);
	const own = packages.length || steps.length ? [] : requirementFix(requirement, host, failed.find(outcome => outcome.fixKey)?.fixKey);
	const fix = status === 'missing' || status === 'unknown' ? [...new Set([...steps, ...own])] : [];
	const optional = requirement.optional === true;
	return { id, title: requirement.title, kind: requirement.kind ?? 'tool', status, optional, ...detail ? { detail } : {}, ...note ? { note } : {}, fix, packages };
}

/** The requirement ids of `task`, including those of the task it is `same` as. */
export function taskRequirements(manifest: Manifest, task: string): string[] {
	const entry = Object.hasOwn(manifest.tasks, task) ? manifest.tasks[task] : undefined;
	if (!entry) {
		throw new Error(`build/prerequisites.json has no task ${task}`);
	}
	return [...new Set([...(entry.same ? taskRequirements(manifest, entry.same) : []), ...entry.requires ?? []])];
}

function appliesHere(requirement: Requirement, host: Host): boolean {
	return !requirement.platforms || requirement.platforms.includes(host.platform as Platform);
}

/** Checks each requirement of `tasks` once and reports every task. */
export async function checkTasks(manifest: Manifest, host: Host, probe: Probe, tasks: readonly string[] = Object.keys(manifest.tasks)): Promise<TaskReport[]> {
	const results = new Map<string, Promise<CheckResult>>();
	const result = (id: string): Promise<CheckResult> => {
		let pending = results.get(id);
		if (!pending) {
			pending = checkRequirement(id, manifest, host, probe);
			results.set(id, pending);
		}
		return pending;
	};
	return Promise.all(tasks.map(async name => {
		const entry = manifest.tasks[name];
		const ids = taskRequirements(manifest, name).filter(id => appliesHere(manifest.requirements[id], host));
		const checked = await Promise.all(ids.map(result));
		const report = { name, description: entry.description, results: checked };
		if (entry.platforms && !entry.platforms.includes(host.platform as Platform)) {
			return { ...report, status: 'unsupported', reason: `${entry.platforms.map(platform => manifest.platforms[platform].name).join(', ')} only` };
		}
		const unsupported = checked.find(item => item.status === 'unsupported');
		if (unsupported) {
			return { ...report, status: 'unsupported', reason: unsupported.detail ?? unsupported.title };
		}
		const missing = checked.filter(item => item.status === 'missing' && !item.optional);
		if (missing.length) {
			return { ...report, status: 'missing', reason: missing.map(item => item.title).join(', ') };
		}
		// Ready only when every requirement it cannot do without was confirmed.
		const unknown = checked.filter(item => item.status === 'unknown' && !item.optional);
		return unknown.length ? { ...report, status: 'unknown', reason: `cannot be checked before it runs: ${unknown.map(item => item.title).join(', ')}` } : { ...report, status: 'ready' };
	}));
}

/** One install command for every missing Linux package, on a known distribution. */
export function installCommand(manifest: Manifest, host: Host, results: readonly CheckResult[]): string | undefined {
	const packages = [...new Set(results.flatMap(result => result.packages))].sort();
	const distribution = host.distribution ? manifest.platforms.linux.distributions?.[host.distribution] : undefined;
	return packages.length && distribution ? `${distribution.install} ${packages.join(' ')}` : undefined;
}

/** Steps fixing `results`, one per line, with Linux packages joined into one command. */
export function fixLines(manifest: Manifest, host: Host, results: readonly CheckResult[]): string[] {
	const lines: string[] = [];
	for (const result of results) {
		lines.push(`  ${result.title}: ${result.detail ?? result.status}`);
		for (const step of result.fix) {
			lines.push(`    ${step}`);
		}
	}
	const install = installCommand(manifest, host, results);
	if (install) {
		lines.push(`  Install the missing packages:`, `    ${install}`);
	}
	return lines;
}

/**
 * Throws, naming what is missing and how to get it, unless `task` can run
 * here. What only the run can confirm, such as a permission the OS asks for
 * when it starts, is named with how to grant it, and the task runs.
 * `VNILINK_SKIP_PREFLIGHT=1` skips the check for setups it misjudges.
 */
export async function assertTaskReady(root: string, task: string, probe: Probe = systemProbe): Promise<void> {
	if (process.env.VNILINK_SKIP_PREFLIGHT === '1') {
		return;
	}
	const manifest = await loadManifest();
	if (!Object.hasOwn(manifest.tasks, task)) {
		return;
	}
	const host = await detectHost(root, manifest, probe);
	const [report] = await checkTasks(manifest, host, probe, [task]);
	for (const result of report.results) {
		if (result.status === 'ready' && result.note) {
			console.warn(`[${task}] ${result.title}: ${result.note}`);
		}
		// What only the run can confirm is said before it, with how to grant it.
		if (report.status === 'unknown' && result.status === 'unknown' && !result.optional) {
			console.warn([`[${task}] ${result.title}: ${result.detail ?? 'cannot be checked here'}`, ...result.fix.map(step => `[${task}]   ${step}`)].join('\n'));
		}
	}
	if (report.status === 'ready' || report.status === 'unknown') {
		if (taskRequirements(manifest, task).includes('rust')) {
			const downloads = await firstBuildDownloads(manifest, host.platform, root, homedir(), probe);
			if (downloads.length) {
				console.log(`[${task}] The first native build downloads, once: ${downloads.map(download => download.what).join('; ')}.`);
			}
		}
		return;
	}
	if (report.status === 'unsupported') {
		throw new Error(`${task} cannot run here: ${report.reason ?? 'unsupported'}`);
	}
	const blocking = report.results.filter(result => result.status === 'missing' && !result.optional);
	throw new Error([
		`${task} needs what this computer is missing:`,
		...fixLines(manifest, host, blocking),
		'`npm run doctor` reports every task; VNILINK_SKIP_PREFLIGHT=1 runs it anyway.',
	].join('\n'));
}

/** What the first native build downloads on `platform`: those whose cache (under `home`) or output (under `root`) is absent. */
export async function firstBuildDownloads(
	manifest: Manifest,
	platform: NodeJS.Platform,
	root: string,
	home: string,
	probe: Probe = systemProbe,
): Promise<Download[]> {
	const downloads: Download[] = [];
	for (const download of manifest.downloads) {
		const paths = pathsOf(platform);
		const present = download.cache ? paths.join(home, download.cache) : paths.join(root, download.into);
		if (download.platforms.includes(platform as Platform) && !await probe.exists(present)) {
			downloads.push(download);
		}
	}
	return downloads;
}

/** Package systems and the OS whose requirements they install. */
const packageSystems = { debian: 'linux', fedora: 'linux', msys2: 'win32' } as const;

export type PackageSystem = keyof typeof packageSystems;

/** Every package of `system` that `tasks` can need, whatever this computer has. */
export function systemPackages(manifest: Manifest, system: PackageSystem, tasks: readonly string[], architecture = 'x64'): string[] {
	const platform = packageSystems[system];
	const packages = new Set<string>();
	for (const id of new Set(tasks.flatMap(task => taskRequirements(manifest, task)))) {
		const requirement = manifest.requirements[id];
		if (requirement.platforms && !requirement.platforms.includes(platform)) {
			continue;
		}
		const { check } = requirement;
		const sources = [...Object.values(check.commands ?? {}), ...Object.values(check['pkg-config'] ?? {}), ...Object.values(check.libraries ?? {}), ...check.packages ? [check.packages] : []];
		for (const source of sources) {
			const names = source[system];
			if (names && (!source.architectures || source.architectures.includes(architecture))) {
				for (const name of names.split(' ')) {
					packages.add(name);
				}
			}
		}
	}
	return [...packages].sort();
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	const [command, system, ...tasks] = process.argv.slice(2);
	if (command !== 'packages' || !Object.hasOwn(packageSystems, system) || !tasks.length) {
		console.error('Usage: node build/lib/prerequisites.ts packages <debian|fedora|msys2> <task>... [--architecture x64|arm64]');
		process.exit(2);
	}
	const index = tasks.indexOf('--architecture');
	const architecture = index >= 0 ? tasks.splice(index, 2)[1] : process.arch;
	console.log(systemPackages(await loadManifest(), system as PackageSystem, tasks, architecture).join('\n'));
}
