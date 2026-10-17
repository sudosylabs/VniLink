import { join } from 'node:path';

// Shared by the one-shot build and the desktop watch workflow.
export function buildOptions(root, { production = false, outputDirectory = join(root, 'out') } = {}) {
	const common = { absWorkingDir: root, tsconfig: join(root, 'tsconfig.json'), bundle: true, ...(production ? { minify: true, keepNames: true, define: { 'process.env.NODE_ENV': '"production"' } } : {}) };
	const node = (entry, output) => ({ ...common, entryPoints: [entry], outfile: join(outputDirectory, output), platform: 'node', format: 'cjs', target: 'node22', external: ['electron'] });
	const browser = (entry, output, extra = {}) => ({ ...common, entryPoints: [entry], outfile: join(outputDirectory, output), platform: 'browser', target: 'chrome144', loader: { '.svg': 'text' }, ...extra });
	return [
		node('src/main.ts', 'main.cjs'),
		node('src/platform/engine/electron-utility/engineMain.ts', 'engine.cjs'),
		node('src/preload.ts', 'preload.cjs'),
		...(!production ? [node('test/smoke/main.ts', 'smoke.cjs')] : []),
		browser('src/renderer.ts', 'renderer/renderer.js'),
		...(!production ? [browser('test/browser/index.ts', 'browser-tests.js', { format: 'iife', globalName: 'browserTests' })] : []),
	];
}

export function resources(root, outputDirectory = join(root, 'out')) {
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
	].map(([source, output]) => ({ source: join(root, source), output: join(outputDirectory, output) }));
}
