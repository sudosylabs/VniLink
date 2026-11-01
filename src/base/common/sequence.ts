/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/


export interface ISpliceable<T> {
	splice(start: number, deleteCount: number, toInsert: readonly T[]): void;
}
