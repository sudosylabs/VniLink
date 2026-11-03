// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The desktop entry of the development Electron. A Wayland session takes a
 * window's icon only from the installed `.desktop` file named by its app id,
 * and portals refuse an app id they cannot resolve to one. The checkout runs
 * as `<appId>.dev` (`src/code/electron-main/launch.ts`), so `desktop:linux`
 * installs that entry and its icon for the current user, as `sandbox:linux`
 * installs the checkout's AppArmor profile; `--remove` takes them away. The
 * session watches these directories, so no cache needs refreshing.
 */
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join, posix } from 'node:path';
import { isLinux } from './platform.ts';
import { loadProduct, type Product } from './product.ts';

/** The id the source checkout runs as: the product's with `.dev`. */
export function developmentDesktopId(product: Product): string {
	return `${product.appId}.dev`;
}

/** Where the entry and its icon go under `dataHome` (`$XDG_DATA_HOME`). */
export function developmentDesktopPaths(product: Product, dataHome: string): { readonly entry: string; readonly icon: string } {
	const id = developmentDesktopId(product);
	return {
		entry: join(dataHome, 'applications', `${id}.desktop`),
		icon: join(dataHome, 'icons/hicolor/scalable/apps', `${id}.svg`),
	};
}

/** Quotes one Exec argument and escapes it as a desktop entry string, as the specification requires. */
function execArgument(value: string): string {
	if (/[\n\r]/.test(value)) {
		throw new Error(`A desktop entry cannot start ${JSON.stringify(value)}`);
	}
	return `"${value.replace(/[\\"`$]/g, character => `\\${character}`).replace(/%/g, '%%').replace(/\\/g, '\\\\')}"`;
}

/** The entry that starts the development Electron in `root` and opens invitations with it. */
export function developmentDesktopEntry(product: Product, root: string): string {
	// The entry is read on Linux, so its paths are Linux paths whichever computer writes it.
	const executable = posix.join(root, '.build/electron', product.platforms.linux.executableName);
	const id = developmentDesktopId(product);
	return [
		'[Desktop Entry]',
		'Type=Application',
		`Name=${product.nameLong} (development)`,
		`Comment=${product.description}`,
		`Exec=${execArgument(executable)} ${execArgument(root)} %U`,
		`Icon=${id}`,
		`StartupWMClass=${id}`,
		`MimeType=${product.invitation.mimeType};`,
		'Categories=Network;',
		'Terminal=false',
		'',
	].join('\n');
}

/** `$XDG_DATA_HOME`, or `~/.local/share` when it is unset or empty. */
export function dataHome(environment: NodeJS.ProcessEnv = process.env): string {
	const configured = environment.XDG_DATA_HOME;
	return configured !== undefined && configured !== '' ? configured : posix.join(homedir(), '.local/share');
}

async function replace(path: string, contents: string | Buffer): Promise<void> {
	await mkdir(dirname(path), { recursive: true });
	const pending = `${path}.tmp`;
	await writeFile(pending, contents);
	await rename(pending, path);
}

/** Writes the development entry and icon under `home`, or removes them. */
export async function writeDevelopmentDesktopEntry(root: string, home: string, remove: boolean, signal: AbortSignal): Promise<string> {
	const product = await loadProduct(root);
	const paths = developmentDesktopPaths(product, home);
	if (remove) {
		await rm(paths.entry, { force: true });
		await rm(paths.icon, { force: true });
		return paths.entry;
	}
	const icon = await readFile(join(root, product.platforms.linux.icon.replace(/\.png$/, '.svg')));
	signal.throwIfAborted();
	await replace(paths.icon, icon);
	await replace(paths.entry, developmentDesktopEntry(product, root));
	return paths.entry;
}

/** The `desktop:linux` task: installs the development entry for this user, or with `--remove` removes it. */
export async function installDevelopmentDesktopEntry(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	if (!isLinux) {
		throw new Error('desktop:linux installs the development desktop entry on Linux');
	}
	const unknown = args.filter(argument => argument !== '--remove');
	if (unknown.length > 0) {
		throw new Error(`desktop:linux takes only --remove, not ${unknown.join(' ')}`);
	}
	const remove = args.includes('--remove');
	const entry = await writeDevelopmentDesktopEntry(root, dataHome(), remove, signal);
	console.log(remove ? `[desktop] Removed ${entry}` : `[desktop] ${entry} names the development Electron in ${root}`);
}
