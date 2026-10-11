import { build } from 'esbuild';
import { cp, mkdir, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
await rm(`${root}out`, { recursive: true, force: true });
await mkdir(`${root}out/renderer`, { recursive: true });
const nodeBuild = (entry, output) => build({ entryPoints: [`${root}${entry}`], outfile: `${root}out/${output}`, bundle: true, platform: 'node', format: 'cjs', target: 'node22', external: ['electron'] });
await Promise.all([
	nodeBuild('src/main.ts', 'main.cjs'),
	nodeBuild('src/preload.ts', 'preload.cjs'),
	nodeBuild('test/smoke/main.ts', 'smoke.cjs'),
	build({ entryPoints: [`${root}src/renderer.ts`], outfile: `${root}out/renderer/renderer.js`, bundle: true, platform: 'browser', target: 'chrome144' }),
	cp(`${root}src/workbench/browser/index.html`, `${root}out/renderer/index.html`),
	cp(`${root}assets/desktop/app-icon.svg`, `${root}out/renderer/app-icon.svg`),
]);
