/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { Event } from '../../../base/common/event.js';
import { isChrome, isEdge, isFirefox, isLinux, isMacintosh, isSafari, isWeb, isWindows, isChromeOS } from '../../../base/common/platform.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';

const CONSTANT_VALUES = new Map<string, boolean>();
CONSTANT_VALUES.set('false', false);
CONSTANT_VALUES.set('true', true);
CONSTANT_VALUES.set('isMac', isMacintosh);
CONSTANT_VALUES.set('isLinux', isLinux);
CONSTANT_VALUES.set('isWindows', isWindows);
CONSTANT_VALUES.set('isChromeOS', isChromeOS);
CONSTANT_VALUES.set('isWeb', isWeb);
CONSTANT_VALUES.set('isMacNative', isMacintosh && !isWeb);
CONSTANT_VALUES.set('isEdge', isEdge);
CONSTANT_VALUES.set('isFirefox', isFirefox);
CONSTANT_VALUES.set('isChrome', isChrome);
CONSTANT_VALUES.set('isSafari', isSafari);

const hasOwnProperty = Object.prototype.hasOwnProperty;

export const enum ContextKeyExprType {
	False = 0,
	True = 1,
	Defined = 2,
	Not = 3,
	Equals = 4,
	NotEquals = 5,
	And = 6,
	Regex = 7,
	NotRegex = 8,
	Or = 9,
	In = 10,
	NotIn = 11,
	Greater = 12,
	GreaterEquals = 13,
	Smaller = 14,
	SmallerEquals = 15,
}

export interface IContextKeyExpression {
	cmp(other: ContextKeyExpression): number;
	equals(other: ContextKeyExpression): boolean;
	evaluate(context: IContext): boolean;
	keys(): string[];
	negate(): ContextKeyExpression;

}

export type ContextKeyExpression = (
	ContextKeyFalseExpr | ContextKeyTrueExpr | ContextKeyDefinedExpr | ContextKeyNotExpr
	| ContextKeyEqualsExpr | ContextKeyNotEqualsExpr | ContextKeyRegexExpr
	| ContextKeyNotRegexExpr | ContextKeyAndExpr | ContextKeyOrExpr | ContextKeyInExpr
	| ContextKeyNotInExpr | ContextKeyGreaterExpr | ContextKeyGreaterEqualsExpr
	| ContextKeySmallerExpr | ContextKeySmallerEqualsExpr
);


/*

Syntax grammar:

```ebnf

expression ::= or

or ::= and { '||' and }*

and ::= term { '&&' term }*

term ::=
	| '!' (KEY | true | false | parenthesized)
	| primary

primary ::=
	| 'true'
	| 'false'
	| parenthesized
	| KEY '=~' REGEX
	| KEY [ ('==' | '!=' | '<' | '<=' | '>' | '>=' | 'not' 'in' | 'in') value ]

parenthesized ::=
	| '(' expression ')'

value ::=
	| 'true'
	| 'false'
	| 'in'      	// we support `in` as a value because there's an extension that uses it, ie "when": "languageId == in"
	| VALUE 		// matched by the same regex as KEY; consider putting the value in single quotes if it's a string (e.g., with spaces)
	| SINGLE_QUOTED_STR
	| EMPTY_STR  	// this allows "when": "foo == " which's used by existing extensions

```
*/

export abstract class ContextKeyExpr {

	public static has(key: string): ContextKeyExpression {
		return ContextKeyDefinedExpr.create(key);
	}
	public static and(...expr: Array<ContextKeyExpression | undefined | null>): ContextKeyExpression | undefined {
		return ContextKeyAndExpr.create(expr, null, true);
	}
}


function cmp(a: ContextKeyExpression, b: ContextKeyExpression): number {
	return a.cmp(b);
}

export class ContextKeyFalseExpr implements IContextKeyExpression {
	public static INSTANCE = new ContextKeyFalseExpr();

	public readonly type = ContextKeyExprType.False;

	protected constructor() {
	}

	public cmp(other: ContextKeyExpression): number {
		return this.type - other.type;
	}

	public equals(other: ContextKeyExpression): boolean {
		return (other.type === this.type);
	}

	public evaluate(context: IContext): boolean {
		return false;
	}

	public keys(): string[] {
		return [];
	}

	public negate(): ContextKeyExpression {
		return ContextKeyTrueExpr.INSTANCE;
	}
}

export class ContextKeyTrueExpr implements IContextKeyExpression {
	public static INSTANCE = new ContextKeyTrueExpr();

	public readonly type = ContextKeyExprType.True;

	protected constructor() {
	}

	public cmp(other: ContextKeyExpression): number {
		return this.type - other.type;
	}

	public equals(other: ContextKeyExpression): boolean {
		return (other.type === this.type);
	}

	public evaluate(context: IContext): boolean {
		return true;
	}

	public keys(): string[] {
		return [];
	}

	public negate(): ContextKeyExpression {
		return ContextKeyFalseExpr.INSTANCE;
	}
}

export class ContextKeyDefinedExpr implements IContextKeyExpression {
	public static create(key: string, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		const constantValue = CONSTANT_VALUES.get(key);
		if (typeof constantValue === 'boolean') {
			return constantValue ? ContextKeyTrueExpr.INSTANCE : ContextKeyFalseExpr.INSTANCE;
		}
		return new ContextKeyDefinedExpr(key, negated);
	}

	public readonly type = ContextKeyExprType.Defined;

	protected constructor(
		readonly key: string,
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp1(this.key, other.key);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		return (!!context.getValue(this.key));
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyNotExpr.create(this.key, this);
		}
		return this.negated;
	}
}

export class ContextKeyEqualsExpr implements IContextKeyExpression {

	public static create(key: string, value: any, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		if (typeof value === 'boolean') {
			return (value ? ContextKeyDefinedExpr.create(key, negated) : ContextKeyNotExpr.create(key, negated));
		}
		const constantValue = CONSTANT_VALUES.get(key);
		if (typeof constantValue === 'boolean') {
			const trueValue = constantValue ? 'true' : 'false';
			return (value === trueValue ? ContextKeyTrueExpr.INSTANCE : ContextKeyFalseExpr.INSTANCE);
		}
		return new ContextKeyEqualsExpr(key, value, negated);
	}

	public readonly type = ContextKeyExprType.Equals;

	private constructor(
		private readonly key: string,
		private readonly value: any,
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp2(this.key, this.value, other.key, other.value);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key && this.value === other.value);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		// Intentional ==
		// eslint-disable-next-line eqeqeq
		return (context.getValue(this.key) == this.value);
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyNotEqualsExpr.create(this.key, this.value, this);
		}
		return this.negated;
	}
}

export class ContextKeyInExpr implements IContextKeyExpression {

	public static create(key: string, valueKey: string): ContextKeyInExpr {
		return new ContextKeyInExpr(key, valueKey);
	}

	public readonly type = ContextKeyExprType.In;
	private negated: ContextKeyExpression | null = null;

	private constructor(
		private readonly key: string,
		private readonly valueKey: string,
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp2(this.key, this.valueKey, other.key, other.valueKey);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key && this.valueKey === other.valueKey);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		const source = context.getValue(this.valueKey);

		const item = context.getValue(this.key);

		if (Array.isArray(source)) {
			// eslint-disable-next-line local/code-no-any-casts
			if (source.includes(item as any)) {
				return true;
			}
			// On Windows, file paths are case-insensitive so file URI
			// comparisons must be done in a case-insensitive manner.
			if (isWindows && typeof item === 'string' && item.startsWith('file:///')) {
				const itemLower = item.toLowerCase();
				return source.some(s => typeof s === 'string' && s.toLowerCase() === itemLower);
			}
			return false;
		}

		if (typeof item === 'string' && typeof source === 'object' && source !== null) {
			if (hasOwnProperty.call(source, item)) {
				return true;
			}
			// On Windows, file paths are case-insensitive so file URI
			// property lookups must be done in a case-insensitive manner.
			if (isWindows && item.startsWith('file:///')) {
				const itemLower = item.toLowerCase();
				return Object.keys(source).some(key => key.toLowerCase() === itemLower);
			}
			return false;
		}
		return false;
	}

	public keys(): string[] {
		return [this.key, this.valueKey];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyNotInExpr.create(this.key, this.valueKey);
		}
		return this.negated;
	}
}

export class ContextKeyNotInExpr implements IContextKeyExpression {

	public static create(key: string, valueKey: string): ContextKeyNotInExpr {
		return new ContextKeyNotInExpr(key, valueKey);
	}

	public readonly type = ContextKeyExprType.NotIn;

	private readonly _negated: ContextKeyInExpr;

	private constructor(
		private readonly key: string,
		private readonly valueKey: string,
	) {
		this._negated = ContextKeyInExpr.create(key, valueKey);
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return this._negated.cmp(other._negated);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return this._negated.equals(other._negated);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		return !this._negated.evaluate(context);
	}

	public keys(): string[] {
		return this._negated.keys();
	}

	public negate(): ContextKeyExpression {
		return this._negated;
	}
}

export class ContextKeyNotEqualsExpr implements IContextKeyExpression {

	public static create(key: string, value: any, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		if (typeof value === 'boolean') {
			if (value) {
				return ContextKeyNotExpr.create(key, negated);
			}
			return ContextKeyDefinedExpr.create(key, negated);
		}
		const constantValue = CONSTANT_VALUES.get(key);
		if (typeof constantValue === 'boolean') {
			const falseValue = constantValue ? 'true' : 'false';
			return (value === falseValue ? ContextKeyFalseExpr.INSTANCE : ContextKeyTrueExpr.INSTANCE);
		}
		return new ContextKeyNotEqualsExpr(key, value, negated);
	}

	public readonly type = ContextKeyExprType.NotEquals;

	private constructor(
		private readonly key: string,
		private readonly value: any,
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp2(this.key, this.value, other.key, other.value);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key && this.value === other.value);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		// Intentional !=
		// eslint-disable-next-line eqeqeq
		return (context.getValue(this.key) != this.value);
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyEqualsExpr.create(this.key, this.value, this);
		}
		return this.negated;
	}
}

export class ContextKeyNotExpr implements IContextKeyExpression {

	public static create(key: string, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		const constantValue = CONSTANT_VALUES.get(key);
		if (typeof constantValue === 'boolean') {
			return (constantValue ? ContextKeyFalseExpr.INSTANCE : ContextKeyTrueExpr.INSTANCE);
		}
		return new ContextKeyNotExpr(key, negated);
	}

	public readonly type = ContextKeyExprType.Not;

	private constructor(
		private readonly key: string,
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp1(this.key, other.key);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		return (!context.getValue(this.key));
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyDefinedExpr.create(this.key, this);
		}
		return this.negated;
	}
}

function withFloatOrStr<T extends ContextKeyExpression>(value: any, callback: (value: number | string) => T): T | ContextKeyFalseExpr {
	if (typeof value === 'string') {
		const n = parseFloat(value);
		if (!isNaN(n)) {
			value = n;
		}
	}
	if (typeof value === 'string' || typeof value === 'number') {
		return callback(value);
	}
	return ContextKeyFalseExpr.INSTANCE;
}

export class ContextKeyGreaterExpr implements IContextKeyExpression {

	public static create(key: string, _value: any, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		return withFloatOrStr(_value, (value) => new ContextKeyGreaterExpr(key, value, negated));
	}

	public readonly type = ContextKeyExprType.Greater;

	private constructor(
		private readonly key: string,
		private readonly value: number | string,
		private negated: ContextKeyExpression | null
	) { }

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp2(this.key, this.value, other.key, other.value);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key && this.value === other.value);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		if (typeof this.value === 'string') {
			return false;
		}
		return (parseFloat(context.getValue<any>(this.key)) > this.value);
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeySmallerEqualsExpr.create(this.key, this.value, this);
		}
		return this.negated;
	}
}

export class ContextKeyGreaterEqualsExpr implements IContextKeyExpression {

	public static create(key: string, _value: any, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		return withFloatOrStr(_value, (value) => new ContextKeyGreaterEqualsExpr(key, value, negated));
	}

	public readonly type = ContextKeyExprType.GreaterEquals;

	private constructor(
		private readonly key: string,
		private readonly value: number | string,
		private negated: ContextKeyExpression | null
	) { }

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp2(this.key, this.value, other.key, other.value);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key && this.value === other.value);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		if (typeof this.value === 'string') {
			return false;
		}
		return (parseFloat(context.getValue<any>(this.key)) >= this.value);
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeySmallerExpr.create(this.key, this.value, this);
		}
		return this.negated;
	}
}

export class ContextKeySmallerExpr implements IContextKeyExpression {

	public static create(key: string, _value: any, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		return withFloatOrStr(_value, (value) => new ContextKeySmallerExpr(key, value, negated));
	}

	public readonly type = ContextKeyExprType.Smaller;

	private constructor(
		private readonly key: string,
		private readonly value: number | string,
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp2(this.key, this.value, other.key, other.value);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key && this.value === other.value);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		if (typeof this.value === 'string') {
			return false;
		}
		return (parseFloat(context.getValue<any>(this.key)) < this.value);
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyGreaterEqualsExpr.create(this.key, this.value, this);
		}
		return this.negated;
	}
}

export class ContextKeySmallerEqualsExpr implements IContextKeyExpression {

	public static create(key: string, _value: any, negated: ContextKeyExpression | null = null): ContextKeyExpression {
		return withFloatOrStr(_value, (value) => new ContextKeySmallerEqualsExpr(key, value, negated));
	}

	public readonly type = ContextKeyExprType.SmallerEquals;

	private constructor(
		private readonly key: string,
		private readonly value: number | string,
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return cmp2(this.key, this.value, other.key, other.value);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return (this.key === other.key && this.value === other.value);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		if (typeof this.value === 'string') {
			return false;
		}
		return (parseFloat(context.getValue<any>(this.key)) <= this.value);
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyGreaterExpr.create(this.key, this.value, this);
		}
		return this.negated;
	}
}

export class ContextKeyRegexExpr implements IContextKeyExpression {

	public readonly type = ContextKeyExprType.Regex;
	private negated: ContextKeyExpression | null = null;

	private constructor(
		private readonly key: string,
		private readonly regexp: RegExp | null
	) {
		//
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		if (this.key < other.key) {
			return -1;
		}
		if (this.key > other.key) {
			return 1;
		}
		const thisSource = this.regexp ? this.regexp.source : '';
		const otherSource = other.regexp ? other.regexp.source : '';
		if (thisSource < otherSource) {
			return -1;
		}
		if (thisSource > otherSource) {
			return 1;
		}
		return 0;
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			const thisSource = this.regexp ? this.regexp.source : '';
			const otherSource = other.regexp ? other.regexp.source : '';
			return (this.key === other.key && thisSource === otherSource);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		const value = context.getValue<any>(this.key);
		return this.regexp ? this.regexp.test(value) : false;
	}

	public keys(): string[] {
		return [this.key];
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			this.negated = ContextKeyNotRegexExpr.create(this);
		}
		return this.negated;
	}
}

export class ContextKeyNotRegexExpr implements IContextKeyExpression {

	public static create(actual: ContextKeyRegexExpr): ContextKeyExpression {
		return new ContextKeyNotRegexExpr(actual);
	}

	public readonly type = ContextKeyExprType.NotRegex;

	private constructor(private readonly _actual: ContextKeyRegexExpr) {
		//
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		return this._actual.cmp(other._actual);
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			return this._actual.equals(other._actual);
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		return !this._actual.evaluate(context);
	}

	public keys(): string[] {
		return this._actual.keys();
	}

	public negate(): ContextKeyExpression {
		return this._actual;
	}
}

export class ContextKeyAndExpr implements IContextKeyExpression {

	public static create(_expr: ReadonlyArray<ContextKeyExpression | null | undefined>, negated: ContextKeyExpression | null, extraRedundantCheck: boolean): ContextKeyExpression | undefined {
		return ContextKeyAndExpr._normalizeArr(_expr, negated, extraRedundantCheck);
	}

	public readonly type = ContextKeyExprType.And;

	private constructor(
		public readonly expr: ContextKeyExpression[],
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		if (this.expr.length < other.expr.length) {
			return -1;
		}
		if (this.expr.length > other.expr.length) {
			return 1;
		}
		for (let i = 0, len = this.expr.length; i < len; i++) {
			const r = cmp(this.expr[i], other.expr[i]);
			if (r !== 0) {
				return r;
			}
		}
		return 0;
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			if (this.expr.length !== other.expr.length) {
				return false;
			}
			for (let i = 0, len = this.expr.length; i < len; i++) {
				if (!this.expr[i].equals(other.expr[i])) {
					return false;
				}
			}
			return true;
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		for (let i = 0, len = this.expr.length; i < len; i++) {
			if (!this.expr[i].evaluate(context)) {
				return false;
			}
		}
		return true;
	}

	private static _normalizeArr(arr: ReadonlyArray<ContextKeyExpression | null | undefined>, negated: ContextKeyExpression | null, extraRedundantCheck: boolean): ContextKeyExpression | undefined {
		const expr: ContextKeyExpression[] = [];
		let hasTrue = false;

		for (const e of arr) {
			if (!e) {
				continue;
			}

			if (e.type === ContextKeyExprType.True) {
				// anything && true ==> anything
				hasTrue = true;
				continue;
			}

			if (e.type === ContextKeyExprType.False) {
				// anything && false ==> false
				return ContextKeyFalseExpr.INSTANCE;
			}

			if (e.type === ContextKeyExprType.And) {
				expr.push(...e.expr);
				continue;
			}

			expr.push(e);
		}

		if (expr.length === 0 && hasTrue) {
			return ContextKeyTrueExpr.INSTANCE;
		}

		if (expr.length === 0) {
			return undefined;
		}

		if (expr.length === 1) {
			return expr[0];
		}

		expr.sort(cmp);

		// eliminate duplicate terms
		for (let i = 1; i < expr.length; i++) {
			if (expr[i - 1].equals(expr[i])) {
				expr.splice(i, 1);
				i--;
			}
		}

		if (expr.length === 1) {
			return expr[0];
		}

		// We must distribute any OR expression because we don't support parens
		// OR extensions will be at the end (due to sorting rules)
		while (expr.length > 1) {
			const lastElement = expr[expr.length - 1];
			if (lastElement.type !== ContextKeyExprType.Or) {
				break;
			}
			// pop the last element
			expr.pop();

			// pop the second to last element
			const secondToLastElement = expr.pop()!;

			const isFinished = (expr.length === 0);

			// distribute `lastElement` over `secondToLastElement`
			const resultElement = ContextKeyOrExpr.create(
				lastElement.expr.map(el => ContextKeyAndExpr.create([el, secondToLastElement], null, extraRedundantCheck)),
				null,
				isFinished
			);

			if (resultElement) {
				expr.push(resultElement);
				expr.sort(cmp);
			}
		}

		if (expr.length === 1) {
			return expr[0];
		}

		// resolve false AND expressions
		if (extraRedundantCheck) {
			for (let i = 0; i < expr.length; i++) {
				for (let j = i + 1; j < expr.length; j++) {
					if (expr[i].negate().equals(expr[j])) {
						// A && !A case
						return ContextKeyFalseExpr.INSTANCE;
					}
				}
			}

			if (expr.length === 1) {
				return expr[0];
			}
		}

		return new ContextKeyAndExpr(expr, negated);
	}

	public keys(): string[] {
		const result: string[] = [];
		for (const expr of this.expr) {
			result.push(...expr.keys());
		}
		return result;
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			const result: ContextKeyExpression[] = [];
			for (const expr of this.expr) {
				result.push(expr.negate());
			}
			this.negated = ContextKeyOrExpr.create(result, this, true)!;
		}
		return this.negated;
	}
}

export class ContextKeyOrExpr implements IContextKeyExpression {

	public static create(_expr: ReadonlyArray<ContextKeyExpression | null | undefined>, negated: ContextKeyExpression | null, extraRedundantCheck: boolean): ContextKeyExpression | undefined {
		return ContextKeyOrExpr._normalizeArr(_expr, negated, extraRedundantCheck);
	}

	public readonly type = ContextKeyExprType.Or;

	private constructor(
		public readonly expr: ContextKeyExpression[],
		private negated: ContextKeyExpression | null
	) {
	}

	public cmp(other: ContextKeyExpression): number {
		if (other.type !== this.type) {
			return this.type - other.type;
		}
		if (this.expr.length < other.expr.length) {
			return -1;
		}
		if (this.expr.length > other.expr.length) {
			return 1;
		}
		for (let i = 0, len = this.expr.length; i < len; i++) {
			const r = cmp(this.expr[i], other.expr[i]);
			if (r !== 0) {
				return r;
			}
		}
		return 0;
	}

	public equals(other: ContextKeyExpression): boolean {
		if (other.type === this.type) {
			if (this.expr.length !== other.expr.length) {
				return false;
			}
			for (let i = 0, len = this.expr.length; i < len; i++) {
				if (!this.expr[i].equals(other.expr[i])) {
					return false;
				}
			}
			return true;
		}
		return false;
	}

	public evaluate(context: IContext): boolean {
		for (let i = 0, len = this.expr.length; i < len; i++) {
			if (this.expr[i].evaluate(context)) {
				return true;
			}
		}
		return false;
	}

	private static _normalizeArr(arr: ReadonlyArray<ContextKeyExpression | null | undefined>, negated: ContextKeyExpression | null, extraRedundantCheck: boolean): ContextKeyExpression | undefined {
		let expr: ContextKeyExpression[] = [];
		let hasFalse = false;

		if (arr) {
			for (let i = 0, len = arr.length; i < len; i++) {
				const e = arr[i];
				if (!e) {
					continue;
				}

				if (e.type === ContextKeyExprType.False) {
					// anything || false ==> anything
					hasFalse = true;
					continue;
				}

				if (e.type === ContextKeyExprType.True) {
					// anything || true ==> true
					return ContextKeyTrueExpr.INSTANCE;
				}

				if (e.type === ContextKeyExprType.Or) {
					expr = expr.concat(e.expr);
					continue;
				}

				expr.push(e);
			}

			if (expr.length === 0 && hasFalse) {
				return ContextKeyFalseExpr.INSTANCE;
			}

			expr.sort(cmp);
		}

		if (expr.length === 0) {
			return undefined;
		}

		if (expr.length === 1) {
			return expr[0];
		}

		// eliminate duplicate terms
		for (let i = 1; i < expr.length; i++) {
			if (expr[i - 1].equals(expr[i])) {
				expr.splice(i, 1);
				i--;
			}
		}

		if (expr.length === 1) {
			return expr[0];
		}

		// resolve true OR expressions
		if (extraRedundantCheck) {
			for (let i = 0; i < expr.length; i++) {
				for (let j = i + 1; j < expr.length; j++) {
					if (expr[i].negate().equals(expr[j])) {
						// A || !A case
						return ContextKeyTrueExpr.INSTANCE;
					}
				}
			}

			if (expr.length === 1) {
				return expr[0];
			}
		}

		return new ContextKeyOrExpr(expr, negated);
	}

	public keys(): string[] {
		const result: string[] = [];
		for (const expr of this.expr) {
			result.push(...expr.keys());
		}
		return result;
	}

	public negate(): ContextKeyExpression {
		if (!this.negated) {
			const result: ContextKeyExpression[] = [];
			for (const expr of this.expr) {
				result.push(expr.negate());
			}

			// We don't support parens, so here we distribute the AND over the OR terminals
			// We always take the first 2 AND pairs and distribute them
			while (result.length > 1) {
				const LEFT = result.shift()!;
				const RIGHT = result.shift()!;

				const all: ContextKeyExpression[] = [];
				for (const left of getTerminals(LEFT)) {
					for (const right of getTerminals(RIGHT)) {
						all.push(ContextKeyAndExpr.create([left, right], null, false)!);
					}
				}

				result.unshift(ContextKeyOrExpr.create(all, null, false)!);
			}

			this.negated = ContextKeyOrExpr.create(result, this, true)!;
		}
		return this.negated;
	}
}

export interface ContextKeyInfo {
	readonly key: string;
	readonly type?: string;
	readonly description?: string;
}

export class RawContextKey<T extends ContextKeyValue> extends ContextKeyDefinedExpr {

	private static _info: ContextKeyInfo[] = [];

	static all(): IterableIterator<ContextKeyInfo> {
		return RawContextKey._info.values();
	}

	private readonly _defaultValue: T | undefined;

	constructor(key: string, defaultValue: T | undefined, metaOrHide?: string | true | { type: string; description: string }) {
		super(key, null);
		this._defaultValue = defaultValue;

		// collect all context keys into a central place
		if (typeof metaOrHide === 'object') {
			RawContextKey._info.push({ ...metaOrHide, key });
		} else if (metaOrHide !== true) {
			RawContextKey._info.push({ key, description: metaOrHide, type: defaultValue !== null && defaultValue !== undefined ? typeof defaultValue : undefined });
		}
	}

	public bindTo(target: IContextKeyService): IContextKey<T> {
		return target.createKey(this.key, this._defaultValue);
	}

	public isEqualTo(value: any): ContextKeyExpression {
		return ContextKeyEqualsExpr.create(this.key, value);
	}
}

export type ContextKeyValue = null | undefined | boolean | number | string
	| Array<null | undefined | boolean | number | string>
	| Record<string, null | undefined | boolean | number | string>;

export interface IContext {
	getValue<T extends ContextKeyValue = ContextKeyValue>(key: string): T | undefined;
}

export interface IContextKey<T extends ContextKeyValue = ContextKeyValue> {
	set(value: T): void;
	reset(): void;
	get(): T | undefined;
}

export const IContextKeyService = createDecorator<IContextKeyService>('contextKeyService');

export interface IReadableSet<T> {
	has(value: T): boolean;
}

export interface IContextKeyChangeEvent {
	affectsSome(keys: IReadableSet<string>): boolean;
	allKeysContainedIn(keys: IReadableSet<string>): boolean;
}

export interface IContextKeyService {
	readonly _serviceBrand: undefined;

	readonly onDidChangeContext: Event<IContextKeyChangeEvent>;

	createKey<T extends ContextKeyValue>(key: string, defaultValue: T | undefined): IContextKey<T>;
	contextMatchesRules(rules: ContextKeyExpression | undefined): boolean;
	getContextKeyValue<T>(key: string): T | undefined;
}

function cmp1(key1: string, key2: string): number {
	if (key1 < key2) {
		return -1;
	}
	if (key1 > key2) {
		return 1;
	}
	return 0;
}

function cmp2(key1: string, value1: any, key2: string, value2: any): number {
	if (key1 < key2) {
		return -1;
	}
	if (key1 > key2) {
		return 1;
	}
	if (value1 < value2) {
		return -1;
	}
	if (value1 > value2) {
		return 1;
	}
	return 0;
}

function getTerminals(node: ContextKeyExpression) {
	if (node.type === ContextKeyExprType.Or) {
		return node.expr;
	}
	return [node];
}
