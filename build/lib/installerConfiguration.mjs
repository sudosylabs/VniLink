/** Native-host installer targets consume an existing, verified Electron application. */
import { packageConfiguration } from './packageConfiguration.mjs';

export function installerConfiguration(root, stage, output, product, manifest) {
	const configuration = packageConfiguration(root, stage, output, product, manifest);
	return {
		...configuration,
		artifactName: product.nameLong + '-${version}-${os}-${arch}.${ext}',
		mac: { ...configuration.mac, target: ['dmg'] },
		dmg: {
			title: product.nameLong,
			format: 'UDZO',
			sign: false,
			writeUpdateInfo: false,
			backgroundColor: '#f5f5f5',
			window: { width: 540, height: 380 },
			contents: [{ x: 140, y: 180, type: 'file' }, { x: 400, y: 180, type: 'link', path: '/Applications' }],
		},
		win: { ...configuration.win, target: ['nsis'] },
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
		linux: {
			...configuration.linux,
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

export function installerExtensions(platform) {
	const extensions = { darwin: ['.dmg'], win32: ['.exe'], linux: ['.deb', '.rpm'] }[platform];
	if (!extensions) {
		throw new Error(`Unsupported installer host: ${platform}`);
	}
	return extensions;
}
