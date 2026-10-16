import { fileURLToPath } from 'node:url';
import { DevelopmentSession } from '../build/lib/development.mjs';

const development = new DevelopmentSession(fileURLToPath(new URL('../', import.meta.url)), process.argv.slice(2));

process.once('SIGINT', () => {
	void development.stop();
});

process.once('SIGTERM', () => {
	void development.stop();
});

await development.start();
