// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Product metadata is separate from runtime state and packager-specific configuration. */
import { readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';

export type ProductPlatform = 'darwin' | 'win32' | 'linux';

export interface ProductTarget {
	readonly architecture: string;
	readonly rustTarget: string;
}

interface PlatformMetadata {
	readonly executableName: string;
	readonly icon: string;
	readonly signing: string;
	readonly targets: readonly ProductTarget[];
}

export interface Product {
	readonly nameShort: string;
	readonly nameLong: string;
	readonly applicationName: string;
	readonly appId: string;
	readonly publisherName: string;
	readonly repositoryUrl: string;
	readonly supportEmail: string;
	readonly invitation: { readonly extension: string; readonly name: string; readonly mimeType: string };
	readonly platforms: {
		readonly darwin: PlatformMetadata & { readonly minimumSystemVersion: string; readonly fallbackIconSource: string };
		readonly win32: PlatformMetadata & { readonly minimumSystemVersion: string };
		readonly linux: PlatformMetadata & {
			readonly desktopFileName: string;
			readonly intendedDistributions: readonly { readonly id: string; readonly minimumVersion: string }[];
		};
	};
}

const rustTargets: Readonly<Record<ProductPlatform, Readonly<Record<string, string>>>> = {
	darwin: { arm64: 'aarch64-apple-darwin', x64: 'x86_64-apple-darwin' },
	win32: { x64: 'x86_64-pc-windows-gnu' },
	linux: { x64: 'x86_64-unknown-linux-gnu' },
};

function requireValue(condition: unknown, message: string): asserts condition {
	if (!condition) {
		throw new Error(`Invalid product metadata: ${message}`);
	}
}

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown): value is string {
	return typeof value === 'string' && value.trim().length > 0;
}

function assetPath(value: unknown): boolean {
	return text(value) && value.startsWith('assets/') && value.split('/').every(segment => segment !== '..' && segment !== '.' && segment.length > 0) && !value.includes('\\');
}

function matches(value: unknown, pattern: RegExp): value is string {
	return typeof value === 'string' && pattern.test(value);
}

/** Validates packaging inputs, not platform runtime support or signing credentials. */
export function validateProduct(product: unknown): Product {
	requireValue(record(product), 'expected an object');
	for (const field of ['nameShort', 'nameLong', 'publisherName']) {
		requireValue(text(product[field]), `${field} is required`);
	}
	requireValue(matches(product.applicationName, /^[a-z][a-z0-9-]*$/), 'invalid applicationName');
	requireValue(matches(product.appId, /^[a-z][a-z0-9]*(\.[a-z][a-z0-9-]*){2,}$/), 'invalid appId');
	requireValue(typeof product.repositoryUrl === 'string' && product.repositoryUrl.startsWith('https://github.com/'), 'expected an HTTPS repositoryUrl');
	requireValue(matches(product.supportEmail, /^[^\s@]+@[^\s@]+\.[^\s@]+$/), 'invalid supportEmail');
	const invitation = product.invitation;
	requireValue(record(invitation) && invitation.extension === 'vnl' && text(invitation.name) && invitation.mimeType === 'application/x-vnilink-invitation', 'invalid invitation association');
	const platforms = product.platforms;
	requireValue(record(platforms) && Object.keys(platforms).sort().join(',') === 'darwin,linux,win32', 'expected darwin, linux and win32 platforms');
	for (const platform of ['darwin', 'win32', 'linux'] as const) {
		const values = platforms[platform];
		requireValue(record(values), `invalid ${platform} metadata`);
		requireValue(matches(values.executableName, /^[A-Za-z][A-Za-z0-9-]*$/), `invalid ${platform} executableName`);
		requireValue(assetPath(values.icon), `invalid ${platform} icon path`);
		requireValue(Array.isArray(values.targets) && values.targets.length > 0, `missing ${platform} targets`);
		const seen = new Set<string>();
		for (const target of values.targets as unknown[]) {
			requireValue(record(target) && typeof target.architecture === 'string' && Object.hasOwn(rustTargets[platform], target.architecture) && rustTargets[platform][target.architecture] === target.rustTarget, `invalid ${platform} architecture/Rust target pair`);
			requireValue(!seen.has(target.architecture), `duplicate ${platform} architecture`);
			seen.add(target.architecture);
		}
	}
	const { darwin, win32, linux } = platforms as Record<ProductPlatform, Record<string, unknown>>;
	requireValue(darwin.signing === 'developer-id-and-notarization' && win32.signing === 'unsigned' && linux.signing === 'unsigned', 'unexpected signing policy');
	requireValue(matches(darwin.minimumSystemVersion, /^\d+\.\d+$/) && Number(darwin.minimumSystemVersion.split('.')[0]) >= 14, 'macOS floor must cover the native macOS 14 APIs');
	requireValue(matches(win32.minimumSystemVersion, /^10\.0\.\d+$/) && Number(win32.minimumSystemVersion.split('.')[2]) >= 19045, 'Windows floor must be at least 10 22H2');
	requireValue(assetPath(darwin.fallbackIconSource), 'invalid macOS fallback artwork');
	requireValue(linux.desktopFileName === `${product.appId}.desktop`, 'Linux desktop identity differs from appId');
	requireValue(Array.isArray(linux.intendedDistributions) && linux.intendedDistributions.length > 0, 'missing intended Linux distributions');
	const distributions = new Set<string>();
	for (const distribution of linux.intendedDistributions as unknown[]) {
		requireValue(record(distribution) && matches(distribution.id, /^[a-z][a-z0-9-]*$/) && matches(distribution.minimumVersion, /^\d+(\.\d+)*$/), 'invalid Linux distribution baseline');
		requireValue(!distributions.has(distribution.id), 'duplicate Linux distribution baseline');
		distributions.add(distribution.id);
	}
	// Every field the type declares was checked above.
	return product as unknown as Product;
}

/** Reads and validates `product.json`. */
export async function readProduct(root: string): Promise<Product> {
	return validateProduct(JSON.parse(await readFile(join(root, 'product.json'), 'utf8')));
}

/** Reads and validates `product.json`, and checks that its artwork exists. */
export async function loadProduct(root: string): Promise<Product> {
	const product = await readProduct(root);
	for (const values of Object.values(product.platforms) as PlatformMetadata[]) {
		const metadata = await stat(join(root, values.icon));
		if (values.icon.endsWith('.icon')) {
			requireValue(metadata.isDirectory(), 'Icon Composer asset must be a directory');
			JSON.parse(await readFile(join(root, values.icon, 'icon.json'), 'utf8'));
		} else {
			requireValue(metadata.isFile(), 'icon must be a file');
		}
	}
	requireValue((await stat(join(root, product.platforms.darwin.fallbackIconSource))).isFile(), 'fallback artwork must be a file');
	return product;
}

/** Validates the product and reports what was checked. */
export async function checkProduct(root: string): Promise<void> {
	const product = await loadProduct(root);
	const count = Object.values(product.platforms).reduce((total, platform: PlatformMetadata) => total + platform.targets.length, 0);
	console.log(`${product.nameLong}: product identity, artwork and ${String(count)} planned native targets verified (runtime compatibility not established).`);
}
