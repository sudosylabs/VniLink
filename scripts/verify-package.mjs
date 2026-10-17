import { fileURLToPath } from 'node:url';
import { PackagedApplication } from '../test/automation/packagedApplication.mjs';

const application = new PackagedApplication(fileURLToPath(new URL('../', import.meta.url)));

process.once('SIGINT', () => {
	void application.stop();
});

process.once('SIGTERM', () => {
	void application.stop();
});

await application.run();
