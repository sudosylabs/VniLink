import electron from 'electron';
import { fileURLToPath } from 'node:url';
import { ProcessRunner } from '../build/lib/processes.mjs';
import { NativeBuild } from '../build/lib/nativeBuild.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));

const runner = new ProcessRunner();

process.once('SIGINT', () => {
	void runner.stop();
});

process.once('SIGTERM', () => {
	void runner.stop();
});

try {
	await new NativeBuild(root, runner).assertFresh();
	await runner.run(electron, [root, ...process.argv.slice(2)], { cwd: root });
} finally {
	await runner.stop();
}
