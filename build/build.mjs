import { fileURLToPath } from 'node:url';
import { ApplicationBuild } from './lib/applicationBuild.mjs';

const builder = new ApplicationBuild(fileURLToPath(new URL('../', import.meta.url)));

process.once('SIGINT', () => {
	void builder.stop();
});

process.once('SIGTERM', () => {
	void builder.stop();
});

await builder.run();
