/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 build/lib/gulp/task.ts.
 *--------------------------------------------------------------------------------------------*/

/*---------------------------------------------------------------------------------------------
 *  Adapted from VS Code build/lib/gulp/task.ts (MIT): the same define/series/parallel
 *  composition without gulp, with an AbortSignal passed to every task.
 *--------------------------------------------------------------------------------------------*/

export interface Task {
	(signal: AbortSignal): Promise<void>;
	taskName?: string;
	displayName?: string;
	_tasks?: readonly Task[];
}

export function series(...tasks: Task[]): Task {
	const result: Task = async signal => {
		for (const task of tasks) {
			signal.throwIfAborted();
			await execute(task, signal);
		}
	};
	result._tasks = tasks;
	return result;
}

/** Runs tasks together; a failure is reported after every task has settled. */
export function parallel(...tasks: Task[]): Task {
	const result: Task = async signal => {
		const results = await Promise.allSettled(tasks.map(task => execute(task, signal)));
		const failure = results.find(value => value.status === 'rejected');
		if (failure) {
			throw failure.reason;
		}
	};
	result._tasks = tasks;
	return result;
}

export function define(name: string, task: Task): Task {
	if (task._tasks) {
		const lastTask = task._tasks[task._tasks.length - 1];
		if (lastTask._tasks || lastTask.taskName) {
			// A composite whose last step is itself composite or named gets its own empty step to carry the name.
			return define(name, series(task, () => Promise.resolve()));
		}
		lastTask.taskName = name;
		task.displayName = name;
		return task;
	}
	task.taskName = name;
	task.displayName = name;
	return task;
}

/** Runs `task` with logging for each named step. */
export async function execute(task: Task, signal: AbortSignal): Promise<void> {
	const name = task.taskName ?? task.displayName ?? '<anonymous>';
	if (task._tasks) {
		await task(signal);
		return;
	}
	const started = Date.now();
	console.log(`[task] Starting ${name}`);
	await task(signal);
	console.log(`[task] Finished ${name} (${String(Date.now() - started)}ms)`);
}
