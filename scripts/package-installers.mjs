import { fileURLToPath } from 'node:url';
import { ApplicationInstallers } from '../build/lib/applicationInstallers.mjs';

const installers = new ApplicationInstallers(fileURLToPath(new URL('../', import.meta.url)));

process.once('SIGINT', () => {
	void installers.stop();
});

process.once('SIGTERM', () => {
	void installers.stop();
});

await installers.run();
