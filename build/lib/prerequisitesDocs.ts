// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The per-OS setup section of the contributor guide, generated from
 * `build/prerequisites.json` so the instructions and the checks cannot
 * disagree. `node build/run.ts docs:prerequisites` rewrites it; `--check`
 * (and the tooling tests) fail when it is stale.
 */
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { loadManifest, rustChannel, systemPackages, taskRequirements, type Manifest, type Platform, type Requirement } from './prerequisites.ts';

const start = '<!-- prerequisites:start -->';

const end = '<!-- prerequisites:end -->';

/** What a contributor installs first: enough to build, run and test the app. */
const coreTasks = ['build', 'start', 'test:ui', 'test:rust'];

/** Tasks whose extra needs are listed separately. */
const otherTasks = ['test:linux', 'measure:impaired', 'measure:impaired:linux', 'measure:latency', 'package:installers', 'assets:tray', 'sandbox:linux'];

function applies(requirement: Requirement, platform: Platform): boolean {
	return !requirement.platforms || requirement.platforms.includes(platform);
}

/** How to install `requirement` on macOS or Windows, from its fix or its commands' sources. */
function instruction(manifest: Manifest, id: string, requirement: Requirement, platform: Platform): string {
	if (requirement.prepared) {
		return '`npm ci` prepares it.';
	}
	if (id === 'node') {
		return manifest.node.install;
	}
	if (id === 'rust') {
		return manifest.rust.install;
	}
	const fix = requirement.fix?.[platform] ?? requirement.fix?.default;
	if (fix) {
		return fix.replace('{msys2}', requirement.check.packages?.msys2 ?? '');
	}
	const steps = new Set(Object.values(requirement.check.commands ?? {}).map(source => platform === 'linux' ? undefined : source[platform]).filter(Boolean));
	return [...steps].join('; ');
}

function sentence(text: string): string {
	return text.endsWith('.') ? text : `${text}.`;
}

function requirementLines(manifest: Manifest, ids: readonly string[], platform: Platform): string[] {
	const lines: string[] = [];
	for (const id of ids) {
		const requirement = manifest.requirements[id];
		if (id === 'platform' || !applies(requirement, platform)) {
			continue;
		}
		const purpose = requirement.purpose ? ` ${requirement.purpose}` : '';
		const optional = requirement.optional ? ' (optional)' : '';
		const how = instruction(manifest, id, requirement, platform);
		lines.push(`- **${requirement.title}**${optional}:${purpose}${how ? ` ${sentence(how.charAt(0).toUpperCase() + how.slice(1))}` : ''}`);
	}
	return lines;
}

/** Linux: the requirement's own per-distribution fix, else its packages on each distribution. */
function linuxInstruction(manifest: Manifest, requirement: Requirement): string {
	const distributions = Object.entries(manifest.platforms.linux.distributions ?? {}) as ['debian' | 'fedora', { readonly name: string }][];
	const parts = distributions.map(([system, distribution]) => {
		const fix = requirement.fix?.[system];
		if (fix) {
			return `${distribution.name}: ${fix.replace(/\.$/, '')}`;
		}
		const { check } = requirement;
		const sources = [...Object.values(check.commands ?? {}), ...check.packages ? [check.packages] : []];
		const packages = [...new Set(sources.map(source => source[system]).filter(Boolean))];
		return packages.length ? `${distribution.name}: \`${packages.join(' ')}\`` : '';
	}).filter(Boolean);
	return parts.length ? parts.join('; ') : requirement.fix?.default ?? '';
}

function unique(ids: readonly string[]): string[] {
	return [...new Set(ids)];
}

function architectureLine(manifest: Manifest, platform: Platform): string {
	const entry = manifest.platforms[platform];
	return Object.entries(entry.architectures)
		.map(([architecture, value]) => `${architecture} ${value.status}${value.reason ? ` (${value.reason})` : ''}`)
		.join('; ');
}

function otherTaskLines(manifest: Manifest, platform: Platform, core: ReadonlySet<string>): string[] {
	const lines: string[] = [];
	for (const task of otherTasks) {
		const entry = manifest.tasks[task];
		if (entry.platforms && !entry.platforms.includes(platform)) {
			continue;
		}
		const extra = taskRequirements(manifest, task).filter(id => !core.has(id) && applies(manifest.requirements[id], platform));
		if (!extra.length) {
			continue;
		}
		const needs = extra.map(id => {
			const requirement = manifest.requirements[id];
			// Joined with semicolons, so no step ends its own sentence.
			const how = (platform === 'linux' ? linuxInstruction(manifest, requirement) : instruction(manifest, id, requirement, platform)).replace(/\.$/, '');
			const title = `${requirement.title}${requirement.optional ? ' (optional)' : ''}`;
			return how ? `${title}: ${how}` : title;
		});
		lines.push(`| \`npm run ${task}\` | ${needs.join('; ')} |`);
	}
	return lines.length ? ['', 'Some tasks need more:', '', '| Task | Also needs |', '| --- | --- |', ...lines] : [];
}

function linuxPackageLines(manifest: Manifest, tasks: readonly string[]): string[] {
	const lines: string[] = [];
	for (const [system, distribution] of Object.entries(manifest.platforms.linux.distributions ?? {})) {
		const packages = systemPackages(manifest, system as 'debian' | 'fedora', tasks);
		lines.push(`${distribution.name} (${distribution.verified}):`, '', '```bash', `${distribution.install} ${packages.join(' ')}`, '```', '');
	}
	return lines;
}

/** The generated section, between its markers. */
export function renderPrerequisites(manifest: Manifest, node: string, rust: string): string {
	const core = unique(coreTasks.flatMap(task => taskRequirements(manifest, task)));
	const coreSet = new Set(core);
	const linux = manifest.platforms.linux;
	const lines = [
		start,
		'<!-- Generated from build/prerequisites.json by `node build/run.ts docs:prerequisites`; edit the manifest, not this section. -->',
		'',
		`Every OS needs Node.js ${node} (\`.nvmrc\`; npm refuses other versions) and Rust ${rust} through rustup (\`rust-toolchain.toml\`). `
		+ 'After installing what your OS lists below, `npm ci` installs the dependencies and prepares the Rust toolchain and the development Electron, '
		+ 'and `npm run doctor` reports what each task still needs. Every task checks its own needs before it starts.',
		'',
		'### macOS',
		'',
		`Architectures: ${architectureLine(manifest, 'darwin')}.`,
		'',
		...requirementLines(manifest, core, 'darwin'),
		...otherTaskLines(manifest, 'darwin', coreSet),
		'',
		'### Windows',
		'',
		`Architectures: ${architectureLine(manifest, 'win32')}. VniLink builds with the GNU toolchain (\`${manifest.platforms.win32.rustTarget ?? ''}\`, MSYS2's MinGW-w64); `
		+ 'Visual Studio and the MSVC toolchain are not needed when rustup\'s host is GNU.',
		'',
		...requirementLines(manifest, core, 'win32'),
		...otherTaskLines(manifest, 'win32', coreSet),
		'',
		'### Linux',
		'',
		`Architectures: ${architectureLine(manifest, 'linux')}. Distributions: ${Object.values(linux.distributions ?? {}).map(entry => entry.name).join(', ')}; `
		+ `others are unverified, and the doctor names the pkg-config modules, libraries and commands to find. ${linux.desktops?.reason ?? ''}`,
		'',
		...linuxPackageLines(manifest, coreTasks),
		...requirementLines(manifest, core.filter(id => {
			const requirement = manifest.requirements[id];
			return Boolean(requirement.check.custom && requirement.fix) && !['node', 'rust', 'platform'].includes(id);
		}), 'linux'),
		'- **UI tests without a display**: `scripts/test.sh` runs them on Xvfb with Openbox and a temporary keyring.',
		...otherTaskLines(manifest, 'linux', coreSet),
		'',
		'The first native build downloads, once:',
		'',
		...manifest.downloads.map(download => `- ${download.platforms.map(platform => manifest.platforms[platform].name).join(', ')}: ${download.what}, into \`${download.into}\`.`),
		end,
	];
	return lines.join('\n');
}

async function rendered(root: string): Promise<string> {
	const manifest = await loadManifest();
	const node = (await readFile(join(root, manifest.node.versionFile), 'utf8')).trim();
	const rust = rustChannel(await readFile(join(root, manifest.rust.toolchainFile), 'utf8')) ?? 'stable';
	return renderPrerequisites(manifest, node, rust);
}

/** The contributor guide with the section replaced by the current rendering. */
export async function contributingWithPrerequisites(root: string): Promise<{ readonly current: string; readonly updated: string }> {
	const current = await readFile(join(root, 'CONTRIBUTING.md'), 'utf8');
	const from = current.indexOf(start);
	const to = current.indexOf(end);
	if (from < 0 || to < from) {
		throw new Error(`CONTRIBUTING.md has no ${start} … ${end} section`);
	}
	return { current, updated: current.slice(0, from) + await rendered(root) + current.slice(to + end.length) };
}

export async function updatePrerequisiteDocs(root: string, args: readonly string[]): Promise<void> {
	const { current, updated } = await contributingWithPrerequisites(root);
	if (current === updated) {
		return;
	}
	if (args.includes('--check')) {
		throw new Error('CONTRIBUTING.md is out of date with build/prerequisites.json: run `node build/run.ts docs:prerequisites`.');
	}
	await writeFile(join(root, 'CONTRIBUTING.md'), updated);
	console.log('Updated the prerequisites section of CONTRIBUTING.md');
}
