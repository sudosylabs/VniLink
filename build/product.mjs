/** Product metadata is separate from runtime state and packager-specific configuration. */
import { readFile, stat } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const root = new URL('../', import.meta.url);

const rustTargets = {
	darwin: { arm64: 'aarch64-apple-darwin', x64: 'x86_64-apple-darwin' },
	win32: { x64: 'x86_64-pc-windows-gnu' },
	linux: { x64: 'x86_64-unknown-linux-gnu' },
};

function requireValue(condition, message) {
	if (!condition) {
		throw new Error(`Invalid product metadata: ${message}`);
	}
}

function text(value) {
	return typeof value === 'string' && value.trim().length > 0;
}

function assetPath(value) {
	return text(value) && value.startsWith('assets/') && value.split('/').every(segment => segment !== '..' && segment !== '.' && segment.length > 0) && !value.includes('\\');
}

/** Validates packaging inputs, not platform runtime support or signing credentials. */
export function validateProduct(product) {
	requireValue(product && typeof product === 'object', 'expected an object');
	for (const field of ['nameShort', 'nameLong', 'publisherName']) {
		requireValue(text(product[field]), `${field} is required`);
	}
	requireValue(/^[a-z][a-z0-9-]*$/.test(product.applicationName ?? ''), 'invalid applicationName');
	requireValue(/^[a-z][a-z0-9]*(\.[a-z][a-z0-9-]*){2,}$/.test(product.appId ?? ''), 'invalid appId');
	requireValue(typeof product.repositoryUrl === 'string' && product.repositoryUrl.startsWith('https://github.com/'), 'expected an HTTPS repositoryUrl');
	requireValue(typeof product.supportEmail === 'string' && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(product.supportEmail), 'invalid supportEmail');
	requireValue(product.invitation?.extension === 'vnl' && text(product.invitation.name) && product.invitation.mimeType === 'application/x-vnilink-invitation', 'invalid invitation association');
	requireValue(product.platforms && Object.keys(product.platforms).sort().join(',') === 'darwin,linux,win32', 'expected darwin, linux and win32 platforms');
	for (const [platform, values] of Object.entries(product.platforms)) {
		requireValue(values && typeof values === 'object', `invalid ${platform} metadata`);
		requireValue(typeof values.executableName === 'string' && /^[A-Za-z][A-Za-z0-9-]*$/.test(values.executableName), `invalid ${platform} executableName`);
		requireValue(assetPath(values.icon), `invalid ${platform} icon path`);
		requireValue(Array.isArray(values.targets) && values.targets.length > 0, `missing ${platform} targets`);
		const seen = new Set();
		for (const target of values.targets) {
			requireValue(target && typeof target === 'object' && Object.hasOwn(rustTargets[platform], target.architecture) && rustTargets[platform][target.architecture] === target.rustTarget, `invalid ${platform} architecture/Rust target pair`);
			requireValue(!seen.has(target.architecture), `duplicate ${platform} architecture`);
			seen.add(target.architecture);
		}
	}
	const { darwin, win32, linux } = product.platforms;
	requireValue(darwin.signing === 'developer-id-and-notarization' && win32.signing === 'unsigned' && linux.signing === 'unsigned', 'unexpected signing policy');
	requireValue(/^\d+\.\d+$/.test(darwin.minimumSystemVersion ?? '') && Number(darwin.minimumSystemVersion.split('.')[0]) >= 14, 'macOS floor must cover the native macOS 14 APIs');
	requireValue(/^10\.0\.\d+$/.test(win32.minimumSystemVersion ?? '') && Number(win32.minimumSystemVersion.split('.')[2]) >= 19045, 'Windows floor must be at least 10 22H2');
	requireValue(assetPath(darwin.fallbackIconSource), 'invalid macOS fallback artwork');
	requireValue(linux.desktopFileName === `${product.appId}.desktop`, 'Linux desktop identity differs from appId');
	requireValue(Array.isArray(linux.intendedDistributions) && linux.intendedDistributions.length > 0, 'missing intended Linux distributions');
	const distributions = new Set();
	for (const distribution of linux.intendedDistributions) {
		requireValue(distribution && /^[a-z][a-z0-9-]*$/.test(distribution.id ?? '') && /^\d+(\.\d+)*$/.test(distribution.minimumVersion ?? ''), 'invalid Linux distribution baseline');
		requireValue(!distributions.has(distribution.id), 'duplicate Linux distribution baseline');
		distributions.add(distribution.id);
	}
	return product;
}

export async function loadProduct() {
	const product = validateProduct(JSON.parse(await readFile(new URL('product.json', root), 'utf8')));
	for (const values of Object.values(product.platforms)) {
		const icon = new URL(values.icon, root);
		const metadata = await stat(icon);
		if (values.icon.endsWith('.icon')) {
			requireValue(metadata.isDirectory(), 'Icon Composer asset must be a directory');
			JSON.parse(await readFile(new URL(`${values.icon}/icon.json`, root), 'utf8'));
		} else {
			requireValue(metadata.isFile(), 'icon must be a file');
		}
		if (values.fallbackIconSource) {
			requireValue((await stat(new URL(values.fallbackIconSource, root))).isFile(), 'fallback artwork must be a file');
		}
	}
	return product;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	const product = await loadProduct();
	const count = Object.values(product.platforms).reduce((total, platform) => total + platform.targets.length, 0);
	console.log(`${product.nameLong}: product identity, artwork and ${count} planned native targets verified (runtime compatibility not established).`);
}
