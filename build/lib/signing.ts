// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Release signing, opt-in by environment: without its variables a build is
 * unsigned (ad-hoc sealed on macOS) as before, so a contributor's build never
 * looks for credentials. The packager never discovers them either
 * (`assemblyEnvironment`): these functions pass only what they name.
 *
 * macOS: `VNILINK_MACOS_SIGNING_IDENTITY` names a Developer ID Application
 * identity in the keychain and `VNILINK_MACOS_NOTARY_PROFILE` a `notarytool`
 * keychain profile; the application is signed with the hardened runtime after
 * its executable UUIDs are assigned, and the disk image is signed, notarized
 * and stapled.
 *
 * Windows: `VNILINK_WINDOWS_CERTIFICATE_FILE` is a PKCS#12 code-signing
 * certificate and `VNILINK_WINDOWS_CERTIFICATE_PASSWORD` its password; the
 * packager signs the executables and the installer with them.
 */
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import type { ProcessRunner } from './processes.ts';

/** What a packaging manifest records about an artifact's signature. */
export type SigningKind = 'unsigned' | 'developer-id' | 'authenticode';

export interface MacSigning {
	readonly identity: string;
	readonly notaryProfile: string;
}

export interface WindowsSigning {
	readonly certificateFile: string;
	readonly certificatePassword: string;
}

/** The variables that must be set together, or not at all. */
function pair(environment: NodeJS.ProcessEnv, first: string, second: string): readonly [string, string] | undefined {
	const [a, b] = [environment[first], environment[second]];
	if (!a && !b) {
		return undefined;
	}
	if (!a || !b) {
		throw new Error(`${first} and ${second} sign a release together; set both or neither`);
	}
	return [a, b];
}

export function macSigning(environment: NodeJS.ProcessEnv = process.env): MacSigning | undefined {
	const values = pair(environment, 'VNILINK_MACOS_SIGNING_IDENTITY', 'VNILINK_MACOS_NOTARY_PROFILE');
	return values && { identity: values[0], notaryProfile: values[1] };
}

export function windowsSigning(environment: NodeJS.ProcessEnv = process.env): WindowsSigning | undefined {
	const values = pair(environment, 'VNILINK_WINDOWS_CERTIFICATE_FILE', 'VNILINK_WINDOWS_CERTIFICATE_PASSWORD');
	return values && { certificateFile: values[0], certificatePassword: values[1] };
}

/** The signature this host's build will carry, from the environment. */
export function signingKind(platform: NodeJS.Platform, environment: NodeJS.ProcessEnv = process.env): SigningKind {
	if (platform === 'darwin' && macSigning(environment)) {
		return 'developer-id';
	}
	if (platform === 'win32' && windowsSigning(environment)) {
		return 'authenticode';
	}
	return 'unsigned';
}

/** The signatures a manifest from `platform` may record. */
export function signingKinds(platform: NodeJS.Platform): readonly SigningKind[] {
	return platform === 'darwin' ? ['unsigned', 'developer-id'] : platform === 'win32' ? ['unsigned', 'authenticode'] : ['unsigned'];
}

/** The packager's variables that sign Windows executables and installers, given only to its process. */
export function windowsSigningEnvironment(signing: WindowsSigning | undefined): NodeJS.ProcessEnv {
	return signing ? { WIN_CSC_LINK: signing.certificateFile, WIN_CSC_KEY_PASSWORD: signing.certificatePassword } : {};
}

export const entitlements = fileURLToPath(new URL('./macos/entitlements.plist', import.meta.url));

/**
 * Signs `bundle` inside out with the hardened runtime and VniLink's
 * entitlements, through the `@electron/osx-sign` the pinned packager uses,
 * which knows Electron's helpers and frameworks.
 */
export async function signMacApplication(bundle: string, signing: MacSigning): Promise<void> {
	const packager = createRequire(createRequire(import.meta.url).resolve('app-builder-lib/package.json'));
	const { signAsync } = packager('@electron/osx-sign') as { readonly signAsync: (options: Record<string, unknown>) => Promise<void> };
	await signAsync({
		app: bundle,
		identity: signing.identity,
		platform: 'darwin',
		preAutoEntitlements: false,
		optionsForFile: () => ({ hardenedRuntime: true, entitlements }),
	});
}

/** Signs a disk image, has Apple notarize it and staples the ticket to it. */
export async function notarizeDiskImage(processes: Pick<ProcessRunner, 'run'>, image: string, signing: MacSigning): Promise<void> {
	await processes.run('codesign', ['--sign', signing.identity, '--timestamp', image]);
	await processes.run('xcrun', ['notarytool', 'submit', image, '--keychain-profile', signing.notaryProfile, '--wait']);
	await processes.run('xcrun', ['stapler', 'staple', image]);
}
