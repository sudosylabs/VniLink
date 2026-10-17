import { fileURLToPath } from 'node:url';
import { ApplicationPackage } from '../build/lib/applicationPackage.mjs';

const builder = new ApplicationPackage(fileURLToPath(new URL('../', import.meta.url)));

process.once('SIGINT', () => {
	void builder.stop();
});

process.once('SIGTERM', () => {
	void builder.stop();
});

await builder.run();
