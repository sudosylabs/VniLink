/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import type { INodeProcess } from './platform.js';

function _definePolyfillMarks(timeOrigin?: number) {
	const _data: [string?, number?] = [];
	if (typeof timeOrigin === 'number') {
		_data.push('code/timeOrigin', timeOrigin);
	}
	function mark(name: string, markOptions?: { startTime?: number }) {
		_data.push(name, markOptions?.startTime ?? Date.now());
	}
	function getMarks() {
		const result = [];
		for (let i = 0; i < _data.length; i += 2) {
			result.push({
				name: _data[i],
				startTime: _data[i + 1],
			});
		}
		return result;
	}
	function clearMarks(name?: string) {
		if (typeof name === 'undefined') {
			const hasTimeOrigin = _data.length >= 2 && _data[0] === 'code/timeOrigin';
			const timeOriginValue = hasTimeOrigin ? _data[1] : undefined;
			_data.length = 0;
			if (hasTimeOrigin) {
				_data.push('code/timeOrigin', timeOriginValue);
			}
		} else {
			for (let i = _data.length - 2; i >= 0; i -= 2) {
				if (_data[i] === name) {
					_data.splice(i, 2);
				}
			}
		}
	}
	return { mark, getMarks, clearMarks };
}

declare const process: INodeProcess;

interface IPerformanceEntry {
	readonly name: string;
	readonly startTime: number;
}

interface IPerformanceTiming {
	readonly navigationStart?: number;
	readonly redirectStart?: number;
	readonly fetchStart?: number;
}

interface IPerformance {
	mark(name: string, markOptions?: { startTime?: number }): void;
	clearMarks(name?: string): void;
	getEntriesByType(type: string): IPerformanceEntry[];
	readonly timeOrigin: number;
	readonly timing?: IPerformanceTiming;
	readonly nodeTiming?: unknown;
}

declare const performance: IPerformance;

function _define() {

	// Identify browser environment when following property is not present
	// https://nodejs.org/dist/latest-v16.x/docs/api/perf_hooks.html#performancenodetiming
	if (typeof performance === 'object' && typeof performance.mark === 'function' && !performance.nodeTiming) {
		// in a browser context, reuse performance-util

		if (typeof performance.timeOrigin !== 'number' && !performance.timing) {
			// safari & webworker: because there is no timeOrigin and no workaround
			// we use the `Date.now`-based polyfill.
			return _definePolyfillMarks();
		} else {
			// use "native" performance for mark and getMarks
			return {
				mark(name: string, markOptions?: { startTime?: number }) {
					performance.mark(name, markOptions);
				},
				clearMarks(name?: string) {
					performance.clearMarks(name);
				},
				getMarks() {
					let timeOrigin = performance.timeOrigin;
					if (typeof timeOrigin !== 'number') {
						// safari: there is no timerOrigin but in renderers there is the timing-property
						// see https://bugs.webkit.org/show_bug.cgi?id=174862
						const timing = performance.timing;
						timeOrigin = (timing?.navigationStart ?? timing?.redirectStart ?? timing?.fetchStart) ?? 0;
					}
					const result = [{ name: 'code/timeOrigin', startTime: Math.round(timeOrigin) }];
					for (const entry of performance.getEntriesByType('mark')) {
						result.push({
							name: entry.name,
							startTime: Math.round(timeOrigin + entry.startTime)
						});
					}
					return result;
				}
			};
		}
	} else if (typeof process === 'object') {
		// node.js: use the normal polyfill but add the timeOrigin
		// from the node perf_hooks API as very first mark
		const timeOrigin = typeof performance === 'object' ? performance.timeOrigin : undefined;
		return _definePolyfillMarks(timeOrigin);
	} else {
		// unknown environment
		console.trace('perf-util loaded in UNKNOWN environment');
		return _definePolyfillMarks();
	}
}

interface IMarkService {
	mark(name: string, markOptions?: { startTime?: number }): void;
	clearMarks(name?: string): void;
	getMarks(): PerformanceMark[];
}

function isMarkService(value: unknown): value is IMarkService {
	if (typeof value !== 'object' || value === null) {
		return false;
	}
	const candidate = value as { mark?: unknown; clearMarks?: unknown; getMarks?: unknown };
	return typeof candidate.mark === 'function'
		&& typeof candidate.clearMarks === 'function'
		&& typeof candidate.getMarks === 'function';
}

function _factory(sharedObj: object): IMarkService {
	const store = sharedObj as { MonacoPerformanceMarks?: unknown };
	if (isMarkService(store.MonacoPerformanceMarks)) {
		return store.MonacoPerformanceMarks;
	}
	const created = _define() as IMarkService;
	store.MonacoPerformanceMarks = created;
	return created;
}

const perf = _factory(globalThis);

export function mark(name: string, markOptions?: { startTime?: number }): void {
	perf.mark(name, markOptions);
}

/**
 * Clears performance marks. If a name is given, only marks with that exact
 * name are removed. If no name is given, all marks are removed.
 */
export function clearMarks(name?: string): void {
	perf.clearMarks(name);
}

export interface PerformanceMark {
	readonly name: string;
	readonly startTime: number;
}

/**
 * Returns all marks, sorted by `startTime`.
 */
export function getMarks(): PerformanceMark[] {
	return perf.getMarks();
}
