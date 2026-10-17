/** Converts approved staged product inputs to pinned electron-builder v26 options. */
import { join } from 'node:path';

export function packageConfiguration(root, stage, output, product, manifest) {
	const icon = platform => join(stage, 'packaging-resources', product.platforms[platform].icon);
	const resources = [
		{ from: join(stage, 'app/licenses'), to: 'licenses', filter: ['**/*'] },
		{ from: join(stage, 'app/relink'), to: 'relink', filter: ['**/*'] },
	];
	if (manifest.platform === 'linux') {
		resources.push({ from: icon('linux'), to: 'app-icon.png' });
	}
	return {
		appId: product.appId,
		productName: product.nameLong,
		electronVersion: manifest.electronVersion,
		electronDist: join(root, 'node_modules/electron/dist'),
		directories: { app: join(stage, 'app'), output, buildResources: join(stage, 'packaging-resources') },
		files: ['**/*', '!licenses/**', '!relink/**'],
		asar: true,
		asarUnpack: ['native/**', 'engine.cjs'],
		extraResources: resources,
		npmRebuild: false,
		nodeGypRebuild: false,
		publish: null,
		mac: {
			executableName: product.platforms.darwin.executableName,
			icon: icon('darwin'),
			minimumSystemVersion: product.platforms.darwin.minimumSystemVersion,
			helperBundleId: `${product.appId}.helper`,
			identity: null,
			hardenedRuntime: false,
			notarize: false,
			darkModeSupport: true,
			category: 'public.app-category.utilities',
			extendInfo: {
				NSCameraUsageDescription: 'Share your camera with computers you have paired and allowed.',
				NSMicrophoneUsageDescription: 'Share your microphone with computers you have paired and allowed.',
				NSAudioCaptureUsageDescription: 'Share this computer’s sound with computers you have paired and allowed.',
			},
		},
		win: { executableName: product.platforms.win32.executableName, icon: icon('win32'), signExecutable: false },
		linux: { executableName: product.platforms.linux.executableName, icon: icon('linux'), category: 'Network' },
	};
}

/** Never discover signing credentials or enable a Node/inspection launch mode during assembly. */
export function assemblyEnvironment(environment) {
	return Object.fromEntries(Object.entries({ ...environment, CSC_IDENTITY_AUTO_DISCOVERY: 'false' }).filter(([name]) => !/^(CSC_(?!IDENTITY_AUTO_DISCOVERY)|WIN_CSC_|APPLE_|ELECTRON_RUN_AS_NODE$|NODE_OPTIONS$)/.test(name)));
}

export function packagedLayout(directory, platform, product) {
	const app = platform === 'darwin' ? join(directory, `${product.nameLong}.app`, 'Contents') : directory;
	const resources = join(app, platform === 'darwin' ? 'Resources' : 'resources');
	return {
		executable: join(app, ...(platform === 'darwin' ? ['MacOS'] : []), `${product.platforms[platform].executableName}${platform === 'win32' ? '.exe' : ''}`),
		resources,
		asar: join(resources, 'app.asar'),
		native: join(resources, 'app.asar.unpacked/native/vnilink.node'),
	};
}
