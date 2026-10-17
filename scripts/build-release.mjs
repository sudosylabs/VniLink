import { fileURLToPath } from 'node:url';
import { ReleaseBuild } from '../build/lib/releaseBuild.mjs';

const builder = new ReleaseBuild(fileURLToPath(new URL('../', import.meta.url)));

process.once('SIGINT', () => {
	void builder.stop();
});

process.once('SIGTERM', () => {
	void builder.stop();
});

await builder.run();
