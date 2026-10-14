import { join } from 'node:path';

// Shared by the one-shot build and the desktop watch workflow.
export function buildOptions(root) {
	const common = { absWorkingDir: root, tsconfig: join(root, 'tsconfig.json'), bundle: true };
	const node = (entry, output) => ({ ...common, entryPoints: [entry], outfile: join(root, 'out', output), platform: 'node', format: 'cjs', target: 'node22', external: ['electron'] });
	const browser = (entry, output, extra = {}) => ({ ...common, entryPoints: [entry], outfile: join(root, 'out', output), platform: 'browser', target: 'chrome144', loader: { '.svg': 'text' }, ...extra });
	return [
		node('src/main.ts', 'main.cjs'),
		node('src/platform/engine/electron-utility/engineHost.ts', 'engine.cjs'),
		node('src/preload.ts', 'preload.cjs'),
		node('test/smoke/main.ts', 'smoke.cjs'),
		browser('src/renderer.ts', 'renderer/renderer.js'),
		browser('test/browser/icons.ts', 'icon-tests.js', { format: 'iife', globalName: 'iconTests' }),
		browser('test/browser/dataChannels.ts', 'data-channel-tests.js', { format: 'iife', globalName: 'dataChannelTests' }),
		browser('test/browser/tabs.ts', 'tab-tests.js', { format: 'iife', globalName: 'tabTests' }),
	];
}

export function resources(root) {
	return [
		['src/workbench/browser/index.html', 'renderer/index.html'],
		['assets/desktop/app-icon.svg', 'renderer/app-icon.svg'],
		['node_modules/@phosphor-icons/core/LICENSE', 'licenses/phosphor-icons.txt'],
		['LICENSE.vscode.txt', 'licenses/vscode.txt'],
		['ThirdPartyNotices.vscode.txt', 'licenses/vscode-third-party.txt'],
		['assets/licenses', 'licenses/native-codecs'],
		['Cargo.toml', 'relink/source/Cargo.toml'],
		['Cargo.lock', 'relink/source/Cargo.lock'],
		['.cargo/config.toml', 'relink/source/.cargo/config.toml'],
		['rust-toolchain.toml', 'relink/source/rust-toolchain.toml'],
		['crates', 'relink/source/crates'],
		['vendor', 'relink/source/vendor'],
		['.github/workflows/ci.yml', 'relink/source/.github/workflows/ci.yml'],
	].map(([source, output]) => ({ source: join(root, source), output: join(root, 'out', output) }));
}
