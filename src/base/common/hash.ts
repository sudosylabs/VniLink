/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 * Adapted for VniLink: explicit value cases and platform-compatible hash conversion.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------


type NotSyncHashable = ArrayBufferLike | ArrayBufferView;

/**
 * Return a hash value for an object.
 *
 * Note that this should not be used for binary data types. Instead,
 * prefer {@link hashAsync}.
 */
export function hash<T>(obj: T extends NotSyncHashable ? never : T): number {
	return doHash(obj, 0);
}

export function doHash(obj: unknown, hashVal: number): number {
	switch (typeof obj) {
		case 'object':
			if (obj === null) {
				return numberHash(349, hashVal);
			} else if (Array.isArray(obj)) {
				return arrayHash(obj, hashVal);
			}
			return objectHash(obj, hashVal);
		case 'string':
			return stringHash(obj, hashVal);
		case 'boolean':
			return booleanHash(obj, hashVal);
		case 'number':
			return numberHash(obj, hashVal);
		case 'undefined':
			return numberHash(937, hashVal);
		case 'bigint':
		case 'function':
		case 'symbol':
		default:
			return numberHash(617, hashVal);
	}
}

export function numberHash(val: number, initialHashVal: number): number {
	return (((initialHashVal << 5) - initialHashVal) + val) | 0;  // hashVal * 31 + ch, keep as int32
}

function booleanHash(b: boolean, initialHashVal: number): number {
	return numberHash(b ? 433 : 863, initialHashVal);
}

export function stringHash(s: string, hashVal: number) {
	hashVal = numberHash(149417, hashVal);
	for (let i = 0, length = s.length; i < length; i++) {
		hashVal = numberHash(s.charCodeAt(i), hashVal);
	}
	return hashVal;
}

/** Seed mixed in by {@link arrayHash} before hashing the array elements. */
export const ARRAY_HASH_SEED = 104579;

/** Seed mixed in by {@link objectHash} before hashing the object entries. */
export const OBJECT_HASH_SEED = 181387;

function arrayHash(arr: unknown[], initialHashVal: number): number {
	initialHashVal = numberHash(ARRAY_HASH_SEED, initialHashVal);
	return arr.reduce<number>((hashVal, item) => doHash(item, hashVal), initialHashVal);
}

function objectHash(obj: object, initialHashVal: number): number {
	initialHashVal = numberHash(OBJECT_HASH_SEED, initialHashVal);
	return Object.keys(obj).sort().reduce((hashVal, key) => {
		hashVal = stringHash(key, hashVal);
		return doHash((obj as Record<string, unknown>)[key], hashVal);
	}, initialHashVal);
}


