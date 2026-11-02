/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/


export function equals<T>(one: ReadonlyArray<T> | undefined, other: ReadonlyArray<T> | undefined, itemEquals: (a: T, b: T) => boolean = (a, b) => a === b): boolean {
	if (one === other) {
		return true;
	}

	if (!one || !other) {
		return false;
	}

	if (one.length !== other.length) {
		return false;
	}

	for (let i = 0, len = one.length; i < len; i++) {
		if (!itemEquals(one[i], other[i])) {
			return false;
		}
	}

	return true;
}

/**
 * Remove the element at `index` by replacing it with the last element. This is faster than `splice`
 * but changes the order of the array
 */
export function removeFastWithoutKeepingOrder<T>(array: T[], index: number) {
	const last = array.length - 1;
	if (index < last) {
		array[index] = array[last];
	}
	array.pop();
}

/**
 * Performs a binary search algorithm over a sorted array.
 *
 * @param array The array being searched.
 * @param key The value we search for.
 * @param comparator A function that takes two array elements and returns zero
 *   if they are equal, a negative number if the first element precedes the
 *   second one in the sorting order, or a positive number if the second element
 *   precedes the first one.
 * @return See {@link binarySearch2}
 */
export function binarySearch<T>(array: ReadonlyArray<T>, key: T, comparator: (op1: T, op2: T) => number): number {
	return binarySearch2(array.length, i => comparator(array[i], key));
}

/**
 * Performs a binary search algorithm over a sorted collection. Useful for cases
 * when we need to perform a binary search over something that isn't actually an
 * array, and converting data to an array would defeat the use of binary search
 * in the first place.
 *
 * @param length The collection length.
 * @param compareToKey A function that takes an index of an element in the
 *   collection and returns zero if the value at this index is equal to the
 *   search key, a negative number if the value precedes the search key in the
 *   sorting order, or a positive number if the search key precedes the value.
 * @return A non-negative index of an element, if found. If not found, the
 *   result is -(n+1) (or ~n, using bitwise notation), where n is the index
 *   where the key should be inserted to maintain the sorting order.
 */
export function binarySearch2(length: number, compareToKey: (index: number) => number): number {
	let low = 0,
		high = length - 1;

	while (low <= high) {
		const mid = ((low + high) / 2) | 0;
		const comp = compareToKey(mid);
		if (comp < 0) {
			low = mid + 1;
		} else if (comp > 0) {
			high = mid - 1;
		} else {
			return mid;
		}
	}
	return -(low + 1);
}

/**
 * Splits the given items into a list of (non-empty) groups.
 * `shouldBeGrouped` is used to decide if two consecutive items should be in the same group.
 * The order of the items is preserved.
 */
export function* groupAdjacentBy<T>(items: Iterable<T>, shouldBeGrouped: (item1: T, item2: T) => boolean): Iterable<T[]> {
	let currentGroup: T[] | undefined;
	let last: T | undefined;
	for (const item of items) {
		if (last !== undefined && shouldBeGrouped(last, item)) {
			currentGroup!.push(item);
		} else {
			if (currentGroup) {
				yield currentGroup;
			}
			currentGroup = [item];
		}
		last = item;
	}
	if (currentGroup) {
		yield currentGroup;
	}
}

/**
 * @returns New array with all falsy values removed. The original array IS NOT modified.
 */
export function coalesce<T>(array: ReadonlyArray<T | undefined | null>): T[] {
	return array.filter((e): e is T => !!e);
}

/**
 * Removes duplicates from the given array. The optional keyFn allows to specify
 * how elements are checked for equality by returning an alternate value for each.
 */
export function distinct<T>(array: ReadonlyArray<T>, keyFn: (value: T) => unknown = value => value): T[] {
	const seen = new Set<any>();

	return array.filter(element => {
		const key = keyFn(element);
		if (seen.has(key)) {
			return false;
		}
		seen.add(key);
		return true;
	});
}

export function range(to: number): number[];
export function range(from: number, to: number): number[];
export function range(arg: number, to?: number): number[] {
	let from = typeof to === 'number' ? arg : 0;

	if (typeof to === 'number') {
		from = arg;
	} else {
		from = 0;
		to = arg;
	}

	const result: number[] = [];

	if (from <= to) {
		for (let i = from; i < to; i++) {
			result.push(i);
		}
	} else {
		for (let i = from; i > to; i--) {
			result.push(i);
		}
	}

	return result;
}

/**
 * Insert the new items in the array.
 * @param array The original array.
 * @param start The zero-based location in the array from which to start inserting elements.
 * @param newItems The items to be inserted
 */
export function insertInto<T>(array: T[], start: number, newItems: T[]): void {
	const startIdx = getActualStartIndex(array, start);
	const originalLength = array.length;
	const newItemsLength = newItems.length;
	array.length = originalLength + newItemsLength;
	// Move the items after the start index, start from the end so that we don't overwrite any value.
	for (let i = originalLength - 1; i >= startIdx; i--) {
		array[i + newItemsLength] = array[i];
	}

	for (let i = 0; i < newItemsLength; i++) {
		array[i + startIdx] = newItems[i];
	}
}

/**
 * Removes elements from an array and inserts new elements in their place, returning the deleted elements. Alternative to the native Array.splice method, it
 * can only support limited number of items due to the maximum call stack size limit.
 * @param array The original array.
 * @param start The zero-based location in the array from which to start removing elements.
 * @param deleteCount The number of elements to remove.
 * @returns An array containing the elements that were deleted.
 */
export function splice<T>(array: T[], start: number, deleteCount: number, newItems: T[]): T[] {
	const index = getActualStartIndex(array, start);
	let result = array.splice(index, deleteCount);
	if (result === undefined) {
		// see https://bugs.webkit.org/show_bug.cgi?id=261140
		result = [];
	}
	insertInto(array, index, newItems);
	return result;
}

/**
 * Determine the actual start index (same logic as the native splice() or slice())
 * If greater than the length of the array, start will be set to the length of the array. In this case, no element will be deleted but the method will behave as an adding function, adding as many element as item[n*] provided.
 * If negative, it will begin that many elements from the end of the array. (In this case, the origin -1, meaning -n is the index of the nth last element, and is therefore equivalent to the index of array.length - n.) If array.length + start is less than 0, it will begin from index 0.
 * @param array The target array.
 * @param start The operation index.
 */
function getActualStartIndex<T>(array: T[], start: number): number {
	return start < 0 ? Math.max(start + array.length, 0) : Math.min(start, array.length);
}



/**
 * When comparing two values,
 * a negative number indicates that the first value is less than the second,
 * a positive number indicates that the first value is greater than the second,
 * and zero indicates that neither is the case.
*/
export type CompareResult = number;

export namespace CompareResult {
	export function isNeitherLessOrGreaterThan(result: CompareResult): boolean {
		return result === 0;
	}

	export const neitherLessOrGreaterThan = 0;
}

/**
 * A comparator `c` defines a total order `<=` on `T` as following:
 * `c(a, b) <= 0` iff `a` <= `b`.
 * We also have `c(a, b) == 0` iff `c(b, a) == 0`.
*/
export type Comparator<T> = (a: T, b: T) => CompareResult;

export function compareBy<TItem, TCompareBy>(selector: (item: TItem) => TCompareBy, comparator: Comparator<TCompareBy>): Comparator<TItem> {
	return (a, b) => comparator(selector(a), selector(b));
}

export function tieBreakComparators<TItem>(...comparators: Comparator<TItem>[]): Comparator<TItem> {
	return (item1, item2) => {
		for (const comparator of comparators) {
			const result = comparator(item1, item2);
			if (!CompareResult.isNeitherLessOrGreaterThan(result)) {
				return result;
			}
		}
		return CompareResult.neitherLessOrGreaterThan;
	};
}

/**
 * The natural order on numbers.
*/
export const numberComparator: Comparator<number> = (a, b) => a - b;
