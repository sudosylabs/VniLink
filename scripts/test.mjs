import electron from 'electron';
import { fileURLToPath } from 'node:url';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ProcessRunner } from '../build/lib/processes.mjs';
import { NativeBuild } from '../build/lib/nativeBuild.mjs';
import { ScenarioSelection, scenarioNames } from '../test/automation/scenarios.ts';

class SmokeRunner {
	constructor(args) {
		this.args = args;
		this.selection = new ScenarioSelection(args);
		this.processes = new ProcessRunner();
	}

	async run() {
		if (this.selection.list) {
			console.log(scenarioNames.join('\n'));
			return;
		}
		const profile = await mkdtemp(join(tmpdir(), 'vnilink-test-'));
		try {
			await new NativeBuild(fileURLToPath(new URL('../', import.meta.url)), this.processes).assertFresh();
			// Docker's default seccomp policy blocks Chromium namespace creation.
			// This exception applies only to the maintained isolated Linux test image.
			const containerFlags = process.platform === 'linux' && process.env.VNILINK_TEST_CONTAINER === '1' ? ['--no-sandbox'] : [];
			await this.processes.run(electron, [fileURLToPath(new URL('../out/smoke.cjs', import.meta.url)), ...containerFlags, ...this.args], { env: { ...process.env, VNILINK_TEST_PROFILE: profile } });
		} finally {
			await this.processes.stop();
			await rm(profile, { recursive: true, force: true });
		}
	}

	stop() {
		return this.processes.stop();
	}
}

const runner = new SmokeRunner(process.argv.slice(2));

process.once('SIGINT', () => {
	void runner.stop();
});

process.once('SIGTERM', () => {
	void runner.stop();
});

await runner.run();
