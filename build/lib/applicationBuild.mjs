/** Shared named stages for compilation, resource staging and native addon publication. */
import { build } from 'esbuild';
import { cp, mkdir, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { buildOptions, resources } from '../tasks.mjs';
import { ProcessRunner } from './processes.mjs';
import { NativeBuild } from './nativeBuild.mjs';
import { TaskRegistry } from './tasks.mjs';

export class ApplicationBuild {
	constructor(root) {
		this.root = root;
		this.processes = new ProcessRunner();
		this.native = new NativeBuild(root, this.processes);
		this.tasks = new TaskRegistry()
			.register('native:compile', [], () => this.native.build())
			.register('output:prepare', ['native:compile'], async () => {
				await rm(join(root, 'out'), { recursive: true, force: true });
				await Promise.all(['renderer', 'native', 'licenses'].map(name => mkdir(join(root, 'out', name), { recursive: true })));
			})
			.register('typescript:bundle', ['output:prepare'], () => Promise.all(buildOptions(root).map(options => build(options))))
			.register('resources:stage', ['output:prepare'], () => Promise.all(resources(root).map(({ source, output }) => cp(source, output, { recursive: true }))))
			.register('native:stage', ['output:prepare'], () => this.native.stage())
			.register('application:build', ['typescript:bundle', 'resources:stage', 'native:stage'], async () => { });
	}

	async run() {
		try {
			await this.tasks.run('application:build');
		} finally {
			await this.processes.stop();
		}
	}

	stop() {
		return this.processes.stop();
	}
}
