/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

declare const globalThis: { performance: { now(): number } };
const performanceNow = globalThis.performance.now.bind(globalThis.performance);

export class StopWatch {

	private _startTime: number;
	private _stopTime: number;

	private readonly _now: () => number;

	constructor(highResolution?: boolean) {
		this._now = highResolution === false ? Date.now : performanceNow;
		this._startTime = this._now();
		this._stopTime = -1;
	}

	public elapsed(): number {
		if (this._stopTime !== -1) {
			return this._stopTime - this._startTime;
		}
		return this._now() - this._startTime;
	}
}
