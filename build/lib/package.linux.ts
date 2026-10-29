/** The Linux installers: Debian and RPM packages with a desktop entry matching the application id. */
import type { BuilderConfiguration } from './package.ts';
import type { Product } from './product.ts';

export const extensions = ['.deb', '.rpm'];

export function installerOptions(product: Product, base: BuilderConfiguration): Record<string, unknown> {
	return {
		linux: {
			...base.linux,
			target: ['deb', 'rpm'],
			maintainer: `${product.publisherName} <${product.supportEmail}>`,
			vendor: product.publisherName,
			synopsis: 'Connect your computers with VniLink',
			description: 'Cross-platform remote access through paired computers.',
			syncDesktopName: true,
			desktop: { entry: { Name: product.nameLong, StartupWMClass: product.platforms.linux.desktopFileName.replace(/\.desktop$/, '') } },
		},
	};
}
