/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { ITransaction } from '../base.js';
import { subtransaction } from '../transaction.js';
import { EqualityComparer, Event, IDisposable } from '../commonFacade/deps.js';
import { DebugNameData } from '../debugName.js';
import { getLogger } from '../logging/logging.js';
import { BaseObservable } from './baseObservable.js';
import { DebugLocation } from '../debugLocation.js';


export class FromEventObservable<TArgs, T> extends BaseObservable<T> {
	private _value: T | undefined;
	private _hasValue = false;
	private _subscription: IDisposable | undefined;

	constructor(
		private readonly _debugNameData: DebugNameData,
		private readonly event: Event<TArgs>,
		public readonly _getValue: (args: TArgs | undefined) => T,
		private readonly _getTransaction: () => ITransaction | undefined,
		private readonly _equalityComparator: EqualityComparer<T>,
		debugLocation: DebugLocation
	) {
		super(debugLocation);
	}

	private getDebugName(): string | undefined {
		return this._debugNameData.getDebugName(this);
	}

	public get debugName(): string {
		const name = this.getDebugName();
		return 'From Event' + (name ? `: ${name}` : '');
	}

	protected override onFirstObserverAdded(): void {
		this._subscription = this.event(this.handleEvent);
	}

	private readonly handleEvent = (args: TArgs | undefined) => {
		const newValue = this._getValue(args);
		const oldValue = this._value;
		const hadValue = this._hasValue;

		const didChange = !hadValue || !(this._equalityComparator(oldValue!, newValue));
		let didRunTransaction = false;

		if (didChange) {
			this._value = newValue;
			this._hasValue = true;

			if (hadValue) {
				didRunTransaction = true;
				subtransaction(
					this._getTransaction(),
					(tx) => {
						getLogger()?.handleObservableUpdated(this, { oldValue, newValue, change: undefined, didChange, hadValue });

						for (const o of this._observers) {
							tx.updateObserver(o, this);
							o.handleChange(this, undefined);
						}
					},
					() => {
						const name = this.getDebugName();
						return 'Event fired' + (name ? `: ${name}` : '');
					}
				);
			}
		}

		if (!didRunTransaction) {
			getLogger()?.handleObservableUpdated(this, { oldValue, newValue, change: undefined, didChange, hadValue });
		}
	};

	protected override onLastObserverRemoved(): void {
		this._subscription!.dispose();
		this._subscription = undefined;
		this._hasValue = false;
		this._value = undefined;
	}

	public get(): T {
		if (this._subscription) {
			if (!this._hasValue) {
				this.handleEvent(undefined);
			}
			return this._value!;
		} else {
			// no cache, as there are no subscribers to keep it updated
			const value = this._getValue(undefined);
			return value;
		}
	}

	public debugGetState() {
		return { value: this._value, hasValue: this._hasValue };
	}
}
