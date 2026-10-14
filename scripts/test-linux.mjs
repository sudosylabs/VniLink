/** Reproducible Linux validation with a read-only checkout and isolated writable volumes. */
import { fileURLToPath } from 'node:url';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { ProcessRunner } from '../build/lib/processes.mjs';

class LinuxValidation {
	constructor(root) {
		this.root = root;
		this.processes = new ProcessRunner();
	}

	async run() {
		const output = join(this.root, '.build/linux-validation', String(Date.now()));
		const container = `vnilink-validation-${String(process.pid)}-${String(Date.now())}`;
		let launched = false;
		await mkdir(output, { recursive: true });
		try {
			await this.processes.run('docker', ['info', '--format', '{{.OSType}} {{.Architecture}}']);
			await this.processes.run('docker', ['build', '-f', 'build/containers/Dockerfile.linux', '-t', 'vnilink-linux-validation:local', '.'], { cwd: this.root });
			launched = true;
			await this.processes.run('docker', ['run', '--name', container, '--init', '--mount', `type=bind,source=${this.root},target=/source,readonly`, '--mount', `type=bind,source=${output},target=/evidence`, '--mount', 'type=volume,source=vnilink-linux-cargo-registry,target=/usr/local/cargo/registry', '--mount', 'type=volume,source=vnilink-linux-trixie-rust191-target,target=/work/target', '--mount', 'type=volume,source=vnilink-linux-node-modules,target=/work/node_modules', 'vnilink-linux-validation:local']);
		} finally {
			await this.processes.stop();
			if (launched) {
				const cleanup = new ProcessRunner();
				try {
					await cleanup.run('docker', ['rm', '--force', container]);
				} finally {
					await cleanup.stop();
				}
			}
			console.log(`Linux evidence: ${output}`);
		}
	}

	stop() {
		return this.processes.stop();
	}
}

const validation = new LinuxValidation(fileURLToPath(new URL('../', import.meta.url)));

process.once('SIGINT', () => {
	void validation.stop();
});

process.once('SIGTERM', () => {
	void validation.stop();
});

await validation.run();
