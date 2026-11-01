// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IDisposable } from '../../src/base/common/lifecycle.js';

/** Lets every task queued so far, and the promise callbacks they start, run. No clock is involved. */
export function macrotask(): Promise<void> {
	return new Promise<void>(resolve => {
		const channel = new MessageChannel();
		channel.port1.onmessage = () => {
			channel.port1.close();
			resolve();
		};
		channel.port2.postMessage(null);
	});
}

/** Waits for the page to paint `count` frames: what a layout or style change settles on. */
export async function animationFrames(count = 2): Promise<void> {
	for (let frame = 0; frame < count; frame++) {
		await new Promise<void>(resolve => {
			requestAnimationFrame(() => {
				resolve();
			});
		});
	}
}

/**
 * Resolves as soon as `condition` holds, re-checking it after every task rather than after a
 * fixed delay, and fails once `timeout` milliseconds pass without it.
 */
export async function until(condition: () => boolean, message: string, timeout = 5000): Promise<void> {
	const deadline = performance.now() + timeout;
	while (!condition()) {
		if (performance.now() > deadline) {
			throw new Error(message);
		}
		await macrotask();
	}
}

/**
 * Replaces `setTimeout` and `clearTimeout` so a test moves time itself: a debounce that waits
 * half a second runs when the test advances 500 milliseconds, not after them. Intervals and
 * animation frames are left alone.
 */
export class FakeTimers implements IDisposable {
	private readonly real = { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout };
	private readonly timers = new Map<number, { readonly due: number; readonly run: () => void }>();
	private now = 0;
	private next = 1;

	constructor() {
		globalThis.setTimeout = ((handler: TimerHandler, delay = 0, ...args: unknown[]): number => {
			const id = this.next++;
			const run = typeof handler === 'function' ? () => {
				(handler as (...values: unknown[]) => void)(...args);
			} : () => undefined;
			this.timers.set(id, { due: this.now + Math.max(0, delay), run });
			return id;
		}) as typeof setTimeout;
		globalThis.clearTimeout = ((id?: number): void => {
			if (id !== undefined) {
				this.timers.delete(id);
			}
		}) as typeof clearTimeout;
	}

	/** Runs every timer due within `milliseconds`, in order, including ones they start. */
	public async advance(milliseconds: number): Promise<void> {
		const end = this.now + milliseconds;
		await macrotask();
		for (; ;) {
			let earliest: number | undefined;
			for (const [id, timer] of this.timers) {
				if (timer.due <= end && (earliest === undefined || timer.due < (this.timers.get(earliest)?.due ?? Infinity))) {
					earliest = id;
				}
			}
			const timer = earliest === undefined ? undefined : this.timers.get(earliest);
			if (earliest === undefined || timer === undefined) {
				break;
			}
			this.timers.delete(earliest);
			this.now = Math.max(this.now, timer.due);
			timer.run();
			await macrotask();
		}
		this.now = end;
	}

	public dispose(): void {
		globalThis.setTimeout = this.real.setTimeout;
		globalThis.clearTimeout = this.real.clearTimeout;
		this.timers.clear();
	}
}
