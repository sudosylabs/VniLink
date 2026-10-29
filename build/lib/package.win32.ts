/** The Windows installer: a per-user, one-click NSIS setup without elevation. */
import type { BuilderConfiguration } from './package.ts';
import type { Product } from './product.ts';

export const extensions = ['.exe'];

export function installerOptions(product: Product, base: BuilderConfiguration): Record<string, unknown> {
	return {
		win: { ...base.win, target: ['nsis'] },
		nsis: {
			oneClick: true,
			perMachine: false,
			packElevateHelper: false,
			createDesktopShortcut: false,
			createStartMenuShortcut: true,
			shortcutName: product.nameLong,
			deleteAppDataOnUninstall: false,
			runAfterFinish: false,
			differentialPackage: false,
		},
	};
}
