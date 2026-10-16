/** One Cargo invocation shares native dependency builds across the affected test crates. */
import { fileURLToPath } from 'node:url';
import { ProcessRunner } from '../build/lib/processes.mjs';

class RustTestRunner {
	constructor(root) {
		this.root = root;
		this.processes = new ProcessRunner();
	}

	async run() {
		const backend = { darwin: 'vnilink-macos', win32: 'vnilink-windows', linux: 'vnilink-linux' }[process.platform];
		if (!backend) {
			throw new Error('Unsupported native test platform');
		}
		const packages = ['vnilink', 'vnilink-app', 'vnilink-media', 'vnilink-audio', 'vnilink-hevc', 'vnilink-rename', backend, 'vnilink-node'];
		try {
			await this.processes.run('cargo', ['test', '--no-fail-fast', '--locked', '--target-dir', 'target/electron', ...packages.flatMap(name => ['-p', name])], { cwd: this.root });
		} finally {
			await this.processes.stop();
		}
	}

	stop() {
		return this.processes.stop();
	}
}

const runner = new RustTestRunner(fileURLToPath(new URL('../', import.meta.url)));

process.once('SIGINT', () => {
	void runner.stop();
});

process.once('SIGTERM', () => {
	void runner.stop();
});

await runner.run();
