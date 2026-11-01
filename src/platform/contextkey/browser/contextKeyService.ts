/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { Emitter, Event, PauseableEmitter } from '../../../base/common/event.js';
import { Iterable } from '../../../base/common/iterator.js';
import { Disposable, DisposableStore, IDisposable, MutableDisposable } from '../../../base/common/lifecycle.js';
import { MarshalledObject } from '../../../base/common/marshalling.js';
import { MarshalledId } from '../../../base/common/marshallingIds.js';
import { cloneAndChange, equals } from '../../../base/common/objects.js';
import { TernarySearchTree } from '../../../base/common/ternarySearchTree.js';
import { URI } from '../../../base/common/uri.js';
import { localize } from '../../../nls.js';
import { CommandsRegistry } from '../../commands/common/commands.js';
import { ConfigurationTarget, IConfigurationService } from '../../configuration/common/configuration.js';
import { ContextKeyExpression, ContextKeyInfo, ContextKeyValue, IContext, IContextKey, IContextKeyChangeEvent, IContextKeyService, IReadableSet, RawContextKey } from '../common/contextkey.js';
import { ServicesAccessor } from '../../instantiation/common/instantiation.js';
import { InputFocusedContext } from '../common/contextkeys.js';
import { mainWindow } from '../../../base/browser/window.js';
import { addDisposableListener, EventType, getActiveWindow, isEditableElement, onDidRegisterWindow, trackFocus } from '../../../base/browser/dom.js';

export class Context implements IContext {

	protected _parent: Context | null;
	protected _value: Record<string, any>;
	protected _id: number;

	constructor(id: number, parent: Context | null) {
		this._id = id;
		this._parent = parent;
		this._value = Object.create(null);
		this._value['_contextId'] = id;
	}

	public get value(): Record<string, any> {
		return { ...this._value };
	}

	public setValue(key: string, value: any): boolean {
		// console.log('SET ' + key + ' = ' + value + ' ON ' + this._id);
		if (!equals(this._value[key], value)) {
			this._value[key] = value;
			return true;
		}
		return false;
	}

	public removeValue(key: string): boolean {
		// console.log('REMOVE ' + key + ' FROM ' + this._id);
		if (key in this._value) {
			delete this._value[key];
			return true;
		}
		return false;
	}

	public getValue<T>(key: string): T | undefined {
		const ret = this._value[key];
		if (typeof ret === 'undefined' && this._parent) {
			return this._parent.getValue<T>(key);
		}
		return ret;
	}
}

class NullContext extends Context {

	static readonly INSTANCE = new NullContext();

	constructor() {
		super(-1, null);
	}

	public override setValue(key: string, value: any): boolean {
		return false;
	}

	public override removeValue(key: string): boolean {
		return false;
	}

	public override getValue<T>(key: string): T | undefined {
		return undefined;
	}
}

class ConfigAwareContextValuesContainer extends Context {
	private static readonly _keyPrefix = 'config.';

	private readonly _values = TernarySearchTree.forConfigKeys<any>();
	private readonly _listener: IDisposable;

	constructor(
		id: number,
		private readonly _configurationService: IConfigurationService,
		emitter: Emitter<IContextKeyChangeEvent>
	) {
		super(id, null);

		this._listener = this._configurationService.onDidChangeConfiguration(event => {
			if (event.source === ConfigurationTarget.DEFAULT) {
				// new setting, reset everything
				const allKeys = Array.from(this._values, ([k]) => k);
				this._values.clear();
				emitter.fire(new ArrayContextKeyChangeEvent(allKeys));
			} else {
				const changedKeys: string[] = [];
				for (const configKey of event.affectedKeys) {
					const contextKey = `config.${configKey}`;

					const cachedItems = this._values.findSuperstr(contextKey);
					if (cachedItems !== undefined) {
						changedKeys.push(...Iterable.map(cachedItems, ([key]) => key));
						this._values.deleteSuperstr(contextKey);
					}

					if (this._values.has(contextKey)) {
						changedKeys.push(contextKey);
						this._values.delete(contextKey);
					}
				}

				emitter.fire(new ArrayContextKeyChangeEvent(changedKeys));
			}
		});
	}

	dispose(): void {
		this._listener.dispose();
	}

	override getValue(key: string): any {

		if (key.indexOf(ConfigAwareContextValuesContainer._keyPrefix) !== 0) {
			return super.getValue(key);
		}

		if (this._values.has(key)) {
			return this._values.get(key);
		}

		const configKey = key.substr(ConfigAwareContextValuesContainer._keyPrefix.length);
		const configValue = this._configurationService.getValue(configKey);
		let value: any = undefined;
		switch (typeof configValue) {
			case 'number':
			case 'boolean':
			case 'string':
				value = configValue;
				break;
			default:
				if (Array.isArray(configValue)) {
					value = JSON.stringify(configValue);
				} else {
					value = configValue;
				}
		}

		this._values.set(key, value);
		return value;
	}

	override setValue(key: string, value: any): boolean {
		return super.setValue(key, value);
	}

	override removeValue(key: string): boolean {
		return super.removeValue(key);
	}
}

class ContextKey<T extends ContextKeyValue> implements IContextKey<T> {

	private _service: AbstractContextKeyService;
	private _key: string;
	private _defaultValue: T | undefined;

	constructor(service: AbstractContextKeyService, key: string, defaultValue: T | undefined) {
		this._service = service;
		this._key = key;
		this._defaultValue = defaultValue;
		this.reset();
	}

	public set(value: T): void {
		this._service.setContext(this._key, value);
	}

	public reset(): void {
		if (typeof this._defaultValue === 'undefined') {
			this._service.removeContext(this._key);
		} else {
			this._service.setContext(this._key, this._defaultValue);
		}
	}

	public get(): T | undefined {
		return this._service.getContextKeyValue<T>(this._key);
	}
}

class SimpleContextKeyChangeEvent implements IContextKeyChangeEvent {
	constructor(readonly key: string) { }
	affectsSome(keys: IReadableSet<string>): boolean {
		return keys.has(this.key);
	}
	allKeysContainedIn(keys: IReadableSet<string>): boolean {
		return this.affectsSome(keys);
	}
}

class ArrayContextKeyChangeEvent implements IContextKeyChangeEvent {
	constructor(readonly keys: string[]) { }
	affectsSome(keys: IReadableSet<string>): boolean {
		for (const key of this.keys) {
			if (keys.has(key)) {
				return true;
			}
		}
		return false;
	}
	allKeysContainedIn(keys: IReadableSet<string>): boolean {
		return this.keys.every(key => keys.has(key));
	}
}

class CompositeContextKeyChangeEvent implements IContextKeyChangeEvent {
	constructor(readonly events: IContextKeyChangeEvent[]) { }
	affectsSome(keys: IReadableSet<string>): boolean {
		for (const e of this.events) {
			if (e.affectsSome(keys)) {
				return true;
			}
		}
		return false;
	}
	allKeysContainedIn(keys: IReadableSet<string>): boolean {
		return this.events.every(evt => evt.allKeysContainedIn(keys));
	}
}

export abstract class AbstractContextKeyService extends Disposable implements IContextKeyService {
	declare _serviceBrand: undefined;

	protected _isDisposed: boolean;
	protected _myContextId: number;

	protected _onDidChangeContext = this._register(new PauseableEmitter<IContextKeyChangeEvent>({ merge: input => new CompositeContextKeyChangeEvent(input) }));
	get onDidChangeContext() { return this._onDidChangeContext.event; }

	constructor(myContextId: number) {
		super();
		this._isDisposed = false;
		this._myContextId = myContextId;
	}

	public createKey<T extends ContextKeyValue>(key: string, defaultValue: T | undefined): IContextKey<T> {
		if (this._isDisposed) {
			throw new Error(`AbstractContextKeyService has been disposed`);
		}
		return new ContextKey(this, key, defaultValue);
	}


	public contextMatchesRules(rules: ContextKeyExpression | undefined): boolean {
		if (this._isDisposed) {
			throw new Error(`AbstractContextKeyService has been disposed`);
		}
		const context = this.getContextValuesContainer(this._myContextId);
		const result = (rules ? rules.evaluate(context) : true);
		// console.group(rules.serialize() + ' -> ' + result);
		// rules.keys().forEach(key => { console.log(key, ctx[key]); });
		// console.groupEnd();
		return result;
	}

	public getContextKeyValue<T>(key: string): T | undefined {
		if (this._isDisposed) {
			return undefined;
		}
		return this.getContextValuesContainer(this._myContextId).getValue<T>(key);
	}

	public setContext(key: string, value: any): void {
		if (this._isDisposed) {
			return;
		}
		const myContext = this.getContextValuesContainer(this._myContextId);
		if (!myContext) {
			return;
		}
		if (myContext.setValue(key, value)) {
			this._onDidChangeContext.fire(new SimpleContextKeyChangeEvent(key));
		}
	}

	public removeContext(key: string): void {
		if (this._isDisposed) {
			return;
		}
		if (this.getContextValuesContainer(this._myContextId).removeValue(key)) {
			this._onDidChangeContext.fire(new SimpleContextKeyChangeEvent(key));
		}
	}

	public abstract getContextValuesContainer(contextId: number): Context;

	public override dispose(): void {
		super.dispose();
		this._isDisposed = true;
	}
}

export class ContextKeyService extends AbstractContextKeyService implements IContextKeyService {

	private _lastContextId: number;
	private readonly _contexts = new Map<number, Context>();

	private inputFocusedContext: IContextKey<boolean>;

	constructor(@IConfigurationService configurationService: IConfigurationService) {
		super(0);
		this._lastContextId = 0;
		this.inputFocusedContext = InputFocusedContext.bindTo(this);

		const myContext = this._register(new ConfigAwareContextValuesContainer(this._myContextId, configurationService, this._onDidChangeContext));
		this._contexts.set(this._myContextId, myContext);

		// Uncomment this to see the contexts continuously logged
		// let lastLoggedValue: string | null = null;
		// setInterval(() => {
		// 	let values = Object.keys(this._contexts).map((key) => this._contexts[key]);
		// 	let logValue = values.map(v => JSON.stringify(v._value, null, '\t')).join('\n');
		// 	if (lastLoggedValue !== logValue) {
		// 		lastLoggedValue = logValue;
		// 		console.log(lastLoggedValue);
		// 	}
		// }, 2000);

		this._register(Event.runAndSubscribe(onDidRegisterWindow, ({ window, disposables }) => {
			const onFocusDisposables = disposables.add(new MutableDisposable<DisposableStore>());
			disposables.add(addDisposableListener(window, EventType.FOCUS_IN, () => {
				onFocusDisposables.value = new DisposableStore();
				this.updateInputContextKeys(window.document, onFocusDisposables.value);
			}, true));
		}, { window: mainWindow, disposables: this._store }));
	}

	private updateInputContextKeys(ownerDocument: Document, disposables: DisposableStore): void {

		function activeElementIsInput(): boolean {
			return !!ownerDocument.activeElement && isEditableElement(ownerDocument.activeElement);
		}

		const isInputFocused = activeElementIsInput();
		this.inputFocusedContext.set(isInputFocused);

		if (isInputFocused) {
			const tracker = disposables.add(trackFocus(ownerDocument.activeElement as HTMLElement));
			Event.once(tracker.onDidBlur)(() => {

				// Ensure we are only updating the context key if we are
				// still in the same document that we are tracking. This
				// fixes a race condition in multi-window setups where
				// the blur event arrives in the inactive window overwriting
				// the context key of the active window. This is because
				// blur events from the focus tracker are emitted with a
				// timeout of 0.

				if (getActiveWindow().document === ownerDocument) {
					this.inputFocusedContext.set(activeElementIsInput());
				}

				disposables.clear();
			}, undefined, disposables);
		}
	}

	public getContextValuesContainer(contextId: number): Context {
		if (this._isDisposed) {
			return NullContext.INSTANCE;
		}
		return this._contexts.get(contextId) || NullContext.INSTANCE;
	}
}

export function setContext(accessor: ServicesAccessor, contextKey: any, contextValue: any) {
	const contextKeyService = accessor.get(IContextKeyService);
	contextKeyService.createKey(String(contextKey), stringifyURIs(contextValue));
}

function stringifyURIs(contextValue: any): any {
	return cloneAndChange(contextValue, (obj) => {
		if (typeof obj === 'object' && (<MarshalledObject>obj).$mid === MarshalledId.Uri) {
			return URI.revive(obj).toString();
		}
		if (obj instanceof URI) {
			return obj.toString();
		}
		return undefined;
	});
}

CommandsRegistry.registerCommand('_setContext', setContext);

CommandsRegistry.registerCommand({
	id: 'getContextKeyInfo',
	handler() {
		return [...RawContextKey.all()].sort((a, b) => a.key.localeCompare(b.key));
	},
	metadata: {
		description: localize('getContextKeyInfo', "A command that returns information about context keys"),
		args: []
	}
});

CommandsRegistry.registerCommand('_generateContextKeyInfo', function () {
	const result: ContextKeyInfo[] = [];
	const seen = new Set<string>();
	for (const info of RawContextKey.all()) {
		if (!seen.has(info.key)) {
			seen.add(info.key);
			result.push(info);
		}
	}
	result.sort((a, b) => a.key.localeCompare(b.key));
	console.log(JSON.stringify(result, undefined, 2));
});
