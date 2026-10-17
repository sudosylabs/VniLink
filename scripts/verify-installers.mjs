import { fileURLToPath } from 'node:url';
import { InstallerVerifier } from '../build/lib/installerVerifier.mjs';
import { PackagedApplication } from '../test/automation/packagedApplication.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));

const verifier = new InstallerVerifier(root, new PackagedApplication(root));

process.once('SIGINT', () => {
	void verifier.stop();
});

process.once('SIGTERM', () => {
	void verifier.stop();
});

await verifier.run();
