/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { IObservableWithChange, IObserver } from '../base.js';
import { DebugLocation } from '../debugLocation.js';
import { debugGetObservableGraph } from '../logging/debugGetDependencyGraph.js';
import { getLogger } from '../logging/logging.js';
import type { keepObserved, recomputeInitiallyAndOnChange } from '../utils/utils.js';
import { derivedOpts } from './derived.js';

let _derived: typeof derivedOpts;
/**
 * @internal
 * This is to allow splitting files.
*/
export function _setDerivedOpts(derived: typeof _derived) {
	_derived = derived;
}

let _recomputeInitiallyAndOnChange: typeof recomputeInitiallyAndOnChange;
export function _setRecomputeInitiallyAndOnChange(recomputeInitiallyAndOnChange: typeof _recomputeInitiallyAndOnChange) {
	_recomputeInitiallyAndOnChange = recomputeInitiallyAndOnChange;
}

let _keepObserved: typeof keepObserved;
export function _setKeepObserved(keepObserved: typeof _keepObserved) {
	_keepObserved = keepObserved;
}

let _debugGetObservableGraph: typeof debugGetObservableGraph;
export function _setDebugGetObservableGraph(debugGetObservableGraph: typeof _debugGetObservableGraph) {
	_debugGetObservableGraph = debugGetObservableGraph;
}

export abstract class ConvenientObservable<T, TChange> implements IObservableWithChange<T, TChange> {
	get TChange(): TChange { return null!; }

	public abstract get(): T;

	public reportChanges(): void {
		this.get();
	}

	public abstract addObserver(observer: IObserver): void;
	public abstract removeObserver(observer: IObserver): void;

	public abstract get debugName(): string;
}

export abstract class BaseObservable<T, TChange = void> extends ConvenientObservable<T, TChange> {
	protected readonly _observers = new Set<IObserver>();

	constructor(debugLocation: DebugLocation) {
		super();
		getLogger()?.handleObservableCreated(this, debugLocation);
	}

	public addObserver(observer: IObserver): void {
		const len = this._observers.size;
		this._observers.add(observer);
		if (len === 0) {
			this.onFirstObserverAdded();
		}
		if (len !== this._observers.size) {
			getLogger()?.handleOnListenerCountChanged(this, this._observers.size);
		}
	}

	public removeObserver(observer: IObserver): void {
		const deleted = this._observers.delete(observer);
		if (deleted && this._observers.size === 0) {
			this.onLastObserverRemoved();
		}
		if (deleted) {
			getLogger()?.handleOnListenerCountChanged(this, this._observers.size);
		}
	}

	protected onFirstObserverAdded(): void { }
	protected onLastObserverRemoved(): void { }

	public debugGetObservers() {
		return this._observers;
	}
}
