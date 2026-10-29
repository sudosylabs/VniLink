import { isMacintosh, isWindows } from '../../../base/common/platform.js';

export const windowPlatform = isMacintosh ? 'darwin' : isWindows ? 'win32' : 'linux';

/** `glass` is macOS Liquid Glass (macOS 26 and later); `vibrancy` is its fallback. */
export type WindowBackgroundMaterial = 'none' | 'vibrancy' | 'glass' | 'mica';

export function isWindowBackgroundMaterial(value: unknown): value is WindowBackgroundMaterial {
	return value === 'none' || value === 'vibrancy' || value === 'glass' || value === 'mica';
}

/** Native backdrops are optional; accessibility preferences take precedence. */
export function resolveWindowMaterial(platform: string, osRelease: string, reducedTransparency: boolean, highContrast: boolean): WindowBackgroundMaterial {
	if (reducedTransparency || highContrast) {
		return 'none';
	}
	const version = /^(\d+)\.(\d+)\.(\d+)/.exec(osRelease);
	if (!version) {
		return 'none';
	}
	const major = Number(version[1]);
	// Darwin 25 is macOS 26, the first with Liquid Glass.
	if (platform === 'darwin' && major >= 25) {
		return 'glass';
	}
	if (platform === 'darwin' && major >= 18) {
		return 'vibrancy';
	}
	if (platform === 'win32' && (major > 10 || major === 10 && Number(version[3]) >= 22621)) {
		return 'mica';
	}
	return 'none';
}
