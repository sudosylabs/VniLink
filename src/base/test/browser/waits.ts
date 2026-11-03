// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Waits for what only a rendered page provides: painted frames, and work a
 * real decoder or renderer finishes in its own time. Everything else a test
 * waits for is a promise, an event or virtual time.
 */

/** Resolves after every task queued so far, and the promise callbacks they start, have run. No clock is involved. */
function macrotask(): Promise<void> {
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

/** The real clock, read before a test can replace `performance.now` with virtual time. */
const realNow = performance.now.bind(performance);

/**
 * Resolves as soon as `condition` holds, re-checking it after every task
 * rather than after a fixed delay, and fails with `message` once `limit`
 * real milliseconds pass without it: for a decoder, a canvas or a renderer
 * that reports no event. The limit is real time even under virtual time,
 * and stays below the test's own timeout so the message is the failure.
 */
export async function until(condition: () => boolean, message: string, limit = 4_000): Promise<void> {
	const deadline = realNow() + limit;
	while (!condition()) {
		if (realNow() > deadline) {
			throw new Error(message);
		}
		await macrotask();
	}
}
