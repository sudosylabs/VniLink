/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { IObservableWithChange, IReader } from './base.js';

export interface IChangeTracker<TChangeSummary> {
	createChangeSummary(previousChangeSummary: TChangeSummary | undefined): TChangeSummary;
	handleChange(ctx: IChangeContext, change: TChangeSummary): boolean;
	beforeUpdate?(reader: IReader, change: TChangeSummary): void;
}

export interface IChangeContext {
	readonly changedObservable: IObservableWithChange<any, any>;
	readonly change: unknown;

	/**
	 * Returns if the given observable caused the change.
	 */
	didChange<T, TChange>(observable: IObservableWithChange<T, TChange>): this is { change: TChange };
}
