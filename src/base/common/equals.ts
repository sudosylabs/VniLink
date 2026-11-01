/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/


/*
 * Each function in this file which offers an equality comparison, has an accompanying
 * `*C` variant which returns an EqualityComparer function.
 *
 * The `*C` variant allows for easier composition of equality comparers and improved type-inference.
*/


/** Represents a function that decides if two values are equal. */
export type EqualityComparer<T> = (a: T, b: T) => boolean;

/**
 * Compares two items for equality using strict equality.
*/
export function strictEquals<T>(a: T, b: T): boolean {
	return a === b;
}
