/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import * as arrays from './arrays.js';
import * as types from './types.js';
import * as nls from '../../nls.js';
import type { IAction } from './actions.js';

/**
 * The `unknown` thrown values this module formats. Every member stays
 * `unknown` so narrowing happens at each use site instead of hiding
 * dynamic error shapes behind `any`.
 */
interface IErrorLike {
	readonly code?: unknown;
	readonly errno?: unknown;
	readonly syscall?: unknown;
	readonly message?: unknown;
	readonly stack?: unknown;
	readonly stacktrace?: unknown;
	readonly detail?: unknown;
	readonly error?: unknown;
	readonly exception?: unknown;
}

// Thrown values are arbitrary, so any object is treated as an error record.
// The predicate carries the narrowing; member checks stay at the use sites.
function isErrorRecord(value: object): value is IErrorLike {
	return typeof value === 'object' || typeof value === 'function';
}

function toErrorLike(error: unknown): IErrorLike {
	if (typeof error === 'object' && error !== null && isErrorRecord(error)) {
		return error;
	}
	return {};
}

function exceptionToErrorMessage(exception: unknown, verbose: boolean): string {
	const candidate = toErrorLike(exception);
	const first = stackToString(candidate.stack as string[] | string | undefined);
	const second = stackToString(candidate.stacktrace as string[] | string | undefined);
	const stack = first !== undefined && first !== '' ? first : second;
	if (verbose && stack) {
		return nls.localize('stackTrace.format', '{0}: {1}', detectSystemErrorMessage(exception), stack);
	}
	return detectSystemErrorMessage(exception);
}

function stackToString(stack: string[] | string | undefined): string | undefined {
	if (Array.isArray(stack)) {
		return stack.join('\n');
	}
	return stack;
}

function detectSystemErrorMessage(exception: unknown): string {
	const candidate = toErrorLike(exception);

	// Custom node.js error from us
	if (candidate.code === 'ERR_UNC_HOST_NOT_ALLOWED') {
		return `${candidate.message as string}. Please update the 'security.allowedUNCHosts' setting if you want to allow this host.`;
	}

	// See https://nodejs.org/api/errors.html#errors_class_system_error
	if (typeof candidate.code === 'string' && typeof candidate.errno === 'number' && typeof candidate.syscall === 'string') {
		return nls.localize('nodeExceptionMessage', 'A system error occurred ({0})', candidate.message as string | number | boolean | null | undefined);
	}
	if (candidate.message) {
		return candidate.message as string;
	}
	return nls.localize('error.defaultMessage', 'An unknown error occurred. Please consult the log for more details.');
}

/**
 * Tries to generate a human readable error message out of the error. If the verbose parameter
 * is set to true, the error message will include stacktrace details if provided.
 *
 * @returns A string containing the error message.
 */
export function toErrorMessage(error: unknown = null, verbose = false): string {
	if (!error) {
		return nls.localize('error.defaultMessage', 'An unknown error occurred. Please consult the log for more details.');
	}
	if (Array.isArray(error)) {
		const errors = arrays.coalesce(error);
		const msg = toErrorMessage(errors[0], verbose);
		if (errors.length > 1) {
			return nls.localize('error.moreErrors', '{0} ({1} errors in total)', msg, errors.length);
		}
		return msg;
	}
	if (types.isString(error)) {
		return error;
	}
	const candidate = toErrorLike(error);
	if (candidate.detail) {
		const detail = toErrorLike(candidate.detail);
		if (detail.error) {
			return exceptionToErrorMessage(detail.error, verbose);
		}
		if (detail.exception) {
			return exceptionToErrorMessage(detail.exception, verbose);
		}
	}
	if (candidate.stack) {
		return exceptionToErrorMessage(error, verbose);
	}
	if (candidate.message) {
		return candidate.message as string;
	}
	return nls.localize('error.defaultMessage', 'An unknown error occurred. Please consult the log for more details.');
}

export interface IErrorWithActions extends Error {
	actions: IAction[];
}

export function isErrorWithActions(obj: unknown): obj is IErrorWithActions {
	const candidate = obj as IErrorWithActions | undefined;
	return candidate instanceof Error && Array.isArray(candidate.actions);
}

export function createErrorWithActions(messageOrError: string | Error, actions: IAction[]): IErrorWithActions {
	let error: IErrorWithActions;
	if (typeof messageOrError === 'string') {
		error = new Error(messageOrError) as IErrorWithActions;
	} else {
		error = messageOrError as IErrorWithActions;
	}
	error.actions = actions;
	return error;
}
