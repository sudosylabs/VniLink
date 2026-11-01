/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { decodeKeybinding, Keybinding } from '../../../base/common/keybindings.js';
import { OperatingSystem, OS } from '../../../base/common/platform.js';
import { ContextKeyExpression } from '../../contextkey/common/contextkey.js';
import { Registry } from '../../registry/common/platform.js';
import { DisposableStore, IDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { LinkedList } from '../../../base/common/linkedList.js';

export interface IKeybindingItem {
	keybinding: Keybinding | null;
	command: string | null;
	commandArgs?: any;
	when: ContextKeyExpression | null | undefined;
	weight1: number;
	weight2: number;
	extensionId: string | null;
	isBuiltinExtension: boolean;
}

export interface IKeybindings {
	primary?: number;
	secondary?: number[];
	win?: {
		primary: number;
		secondary?: number[];
	};
	linux?: {
		primary: number;
		secondary?: number[];
	};
	mac?: {
		primary: number;
		secondary?: number[];
	};
}

export interface IKeybindingRule extends IKeybindings {
	id: string;
	weight: number;
	args?: any;
	/**
	 * Keybinding is disabled if expression returns false.
	 */
	when?: ContextKeyExpression | null | undefined;
}

export interface IKeybindingsRegistry {
	registerKeybindingRule(rule: IKeybindingRule): IDisposable;
}

/**
 * Stores all built-in and extension-provided keybindings (but not ones that user defines themselves)
 */
class KeybindingsRegistryImpl implements IKeybindingsRegistry {

	private _coreKeybindings: LinkedList<IKeybindingItem>;
	private _coreKeybindingRules: LinkedList<IKeybindingRule>;
	private _extensionKeybindings: IKeybindingItem[];
	private _cachedMergedKeybindings: IKeybindingItem[] | null;

	constructor() {
		this._coreKeybindings = new LinkedList();
		this._coreKeybindingRules = new LinkedList();
		this._extensionKeybindings = [];
		this._cachedMergedKeybindings = null;
	}

	private static bindToPlatform(kb: IKeybindings, os: OperatingSystem): { primary?: number; secondary?: number[] } {
		if (os === OperatingSystem.Windows) {
			if (kb && kb.win) {
				return kb.win;
			}
		} else if (os === OperatingSystem.Macintosh) {
			if (kb && kb.mac) {
				return kb.mac;
			}
		} else {
			if (kb && kb.linux) {
				return kb.linux;
			}
		}
		return kb;
	}

	/**
	 * Take current platform into account and reduce to primary & secondary.
	 */
	private static bindToCurrentPlatform(kb: IKeybindings): { primary?: number; secondary?: number[] } {
		return KeybindingsRegistryImpl.bindToPlatform(kb, OS);
	}

	public registerKeybindingRule(rule: IKeybindingRule): IDisposable {
		const actualKb = KeybindingsRegistryImpl.bindToCurrentPlatform(rule);
		const result = new DisposableStore();

		if (actualKb && actualKb.primary) {
			const kk = decodeKeybinding(actualKb.primary, OS);
			if (kk) {
				result.add(this._registerDefaultKeybinding(kk, rule.id, rule.args, rule.weight, 0, rule.when));
			}
		}

		if (actualKb && Array.isArray(actualKb.secondary)) {
			for (let i = 0, len = actualKb.secondary.length; i < len; i++) {
				const k = actualKb.secondary[i];
				const kk = decodeKeybinding(k, OS);
				if (kk) {
					result.add(this._registerDefaultKeybinding(kk, rule.id, rule.args, rule.weight, -i - 1, rule.when));
				}
			}
		}

		const removeRule = this._coreKeybindingRules.push(rule);
		result.add(toDisposable(() => { removeRule(); }));

		return result;
	}

	private _registerDefaultKeybinding(keybinding: Keybinding, commandId: string, commandArgs: any, weight1: number, weight2: number, when: ContextKeyExpression | null | undefined): IDisposable {
		const remove = this._coreKeybindings.push({
			keybinding: keybinding,
			command: commandId,
			commandArgs: commandArgs,
			when: when,
			weight1: weight1,
			weight2: weight2,
			extensionId: null,
			isBuiltinExtension: false
		});
		this._cachedMergedKeybindings = null;

		return toDisposable(() => {
			remove();
			this._cachedMergedKeybindings = null;
		});
	}
}
export const KeybindingsRegistry: IKeybindingsRegistry = new KeybindingsRegistryImpl();

// Define extension point ids
export const Extensions = {
	EditorModes: 'platform.keybindingsRegistry'
};
Registry.add(Extensions.EditorModes, KeybindingsRegistry);
