/** Named dependency graph; composition is functional while the registry owns task definitions. */
export class TaskRegistry {
	#tasks = new Map();

	register(name, dependencies, action) {
		if (this.#tasks.has(name)) {
			throw new Error(`Duplicate task: ${name}`);
		}
		this.#tasks.set(name, { dependencies, action });
		return this;
	}

	async run(name) {
		this.#validate(name, new Set(), new Set());
		const pending = new Map();
		const execute = name => {
			if (!pending.has(name)) {
				const task = this.#tasks.get(name);
				pending.set(name, Promise.all(task.dependencies.map(execute)).then(async () => {
					const started = Date.now();
					console.log(`[task] Starting ${name}`);
					await task.action();
					console.log(`[task] Finished ${name} (${Date.now() - started}ms)`);
				}));
			}
			return pending.get(name);
		};
		try {
			await execute(name);
		} catch (error) {
			await Promise.allSettled(pending.values());
			throw error;
		}
	}

	#validate(name, active, visited) {
		if (active.has(name)) {
			throw new Error(`Task dependency cycle at ${name}`);
		}
		if (visited.has(name)) {
			return;
		}
		const task = this.#tasks.get(name);
		if (!task) {
			throw new Error(`Unknown task: ${name}`);
		}
		active.add(name);
		for (const dependency of task.dependencies) {
			this.#validate(dependency, active, visited);
		}
		active.delete(name);
		visited.add(name);
	}
}
