import { isMacintosh, isWindows } from '../../../base/common/platform.js';

export const windowPlatform = isMacintosh ? 'darwin' : isWindows ? 'win32' : 'linux';

export type WindowBackgroundMaterial = 'none' | 'vibrancy' | 'mica';

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
	if (platform === 'darwin' && major >= 18) {
		return 'vibrancy';
	}
	if (platform === 'win32' && (major > 10 || major === 10 && Number(version[3]) >= 22621)) {
		return 'mica';
	}
	return 'none';
}
