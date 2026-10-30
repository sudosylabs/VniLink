// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Runs on `npm install` and `npm ci`, after VS Code's `build/npm/preinstall.ts`:
 * refuses a Node version, package manager or computer the repository cannot
 * build on, and names what the build will need that this computer lacks, with
 * the command that installs it.
 *
 * Imports only Node's own modules. A Node too old to run TypeScript never gets
 * here: `.npmrc` sets `engine-strict`, so npm refuses it with the `engines`
 * range from `package.json`.
 */
import { fileURLToPath } from 'node:url';
import { checkTasks, detectHost, fixLines, loadManifest, systemProbe, type CheckResult } from '../lib/prerequisites.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

function fail(message: string): never {
	console.error(`\x1b[1;31m*** ${message}\x1b[0m`);
	process.exit(1);
}

if (process.env.npm_execpath?.includes('yarn') || process.env.npm_execpath?.includes('pnpm')) {
	fail('Install with npm (npm ci); package-lock.json is the only lockfile.');
}

const manifest = await loadManifest();

const host = await detectHost(root, manifest, systemProbe);

const reports = await checkTasks(manifest, host, systemProbe, ['install', 'build', 'dev']);

const [install] = reports;

if (install.status !== 'ready') {
	const blocking = install.results.filter(result => result.status !== 'ready');
	const node = blocking.find(result => result.id === 'node');
	if (node && process.env.VNILINK_SKIP_NODE_VERSION_CHECK) {
		console.warn(`Ignoring ${node.detail ?? 'the Node version'} (VNILINK_SKIP_NODE_VERSION_CHECK).`);
	} else {
		fail(['VniLink cannot be installed here:', ...fixLines(manifest, host, blocking)].join('\n'));
	}
}

// The build and the app check these again before they run; here a fresh
// clone learns all of them at once, without stopping the install.
const missing = new Map<string, CheckResult>();

for (const report of reports.slice(1)) {
	for (const result of report.results) {
		if (result.status === 'missing' && !result.optional) {
			missing.set(result.id, result);
		}
	}
}

const platform = install.results.find(result => result.id === 'platform');

if (platform?.status === 'ready' && platform.note) {
	console.warn(`\x1b[33mNote: ${platform.note}\x1b[0m`);
}

if (missing.size) {
	console.warn([
		'\x1b[33mBuilding and running VniLink needs what this computer is missing:',
		...fixLines(manifest, host, [...missing.values()]),
		'`npm run doctor` reports every task.\x1b[0m',
	].join('\n'));
}
