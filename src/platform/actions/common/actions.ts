/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { IAction, SubmenuAction } from '../../../base/common/actions.js';
import { Event, MicrotaskEmitter } from '../../../base/common/event.js';
import { DisposableStore, dispose, IDisposable, markAsSingleton, toDisposable } from '../../../base/common/lifecycle.js';
import { LinkedList } from '../../../base/common/linkedList.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { ICommandAction, ICommandActionTitle, Icon, ILocalizedString } from '../../action/common/action.js';
import { CommandsRegistry, ICommandService } from '../../commands/common/commands.js';
import { ContextKeyExpr, ContextKeyExpression, IContextKeyService } from '../../contextkey/common/contextkey.js';
import { createDecorator, ServicesAccessor } from '../../instantiation/common/instantiation.js';
import { IKeybindingRule, KeybindingsRegistry } from '../../keybinding/common/keybindingsRegistry.js';

export interface IMenuItem {
	command: ICommandAction;
	alt?: ICommandAction;
	/**
	 * Menu item is hidden if this expression returns false.
	 */
	when?: ContextKeyExpression;
	group?: 'navigation' | string;
	order?: number;
	isHiddenByDefault?: boolean;
}

export interface ISubmenuItem {
	title: string | ICommandActionTitle;
	submenu: MenuId;
	when?: ContextKeyExpression;
	group?: 'navigation' | string;
	order?: number;
}

export function isIMenuItem(item: unknown): item is IMenuItem {
	return (item as IMenuItem).command !== undefined;
}

export function isISubmenuItem(item: unknown): item is ISubmenuItem {
	return (item as ISubmenuItem).submenu !== undefined;
}

export class MenuId {

	private static readonly _instances = new Map<string, MenuId>();

	static readonly CommandPalette = new MenuId('CommandPalette');

	readonly id: string;

	/**
	 * Create a new `MenuId` with the unique identifier. Will throw if a menu
	 * with the identifier already exists, use `MenuId.for(ident)` or a unique
	 * identifier
	 */
	constructor(identifier: string) {
		if (MenuId._instances.has(identifier)) {
			throw new TypeError(`MenuId with identifier '${identifier}' already exists. Use MenuId.for(ident) or a unique identifier`);
		}
		MenuId._instances.set(identifier, this);
		this.id = identifier;
	}
}

export interface IMenuActionOptions {
	arg?: unknown;
	args?: unknown[];
	shouldForwardArgs?: boolean;
	renderShortTitle?: boolean;
}

export interface IMenuChangeEvent {
	readonly menu: IMenu;
	readonly isStructuralChange: boolean;
	readonly isToggleChange: boolean;
	readonly isEnablementChange: boolean;
}

export interface IMenu extends IDisposable {
	readonly onDidChange: Event<IMenuChangeEvent>;
	getActions(options?: IMenuActionOptions): [string, Array<MenuItemAction | SubmenuItemAction>][];
}

export const IMenuService = createDecorator<IMenuService>('menuService');

export interface IMenuCreateOptions {
	emitEventsForSubmenuChanges?: boolean;
	eventDebounceDelay?: number;
}

export interface IMenuService {

	readonly _serviceBrand: undefined;

	/**
	 * Consider using getMenuActions if you don't need to listen to events.
	 *
	 * Create a new menu for the given menu identifier. A menu sends events when it's entries
	 * have changed (placement, enablement, checked-state). By default it does not send events for
	 * submenu entries. That is more expensive and must be explicitly enabled with the
	 * `emitEventsForSubmenuChanges` flag.
	 */
	createMenu(id: MenuId, contextKeyService: IContextKeyService, options?: IMenuCreateOptions): IMenu;

	/**
	 * Gets the actions for the current context without creating a menu.
	 */
	getMenuActions(id: MenuId, contextKeyService: IContextKeyService, options?: IMenuActionOptions): [string, Array<MenuItemAction | SubmenuItemAction>][];
}

type ICommandsMap = Map<string, ICommandAction>;

export interface IMenuRegistryChangeEvent {
	has(id: MenuId): boolean;
}

class MenuRegistryChangeEvent {

	private static _all = new Map<MenuId, MenuRegistryChangeEvent>();

	static for(id: MenuId): MenuRegistryChangeEvent {
		let value = this._all.get(id);
		if (!value) {
			value = new MenuRegistryChangeEvent(id);
			this._all.set(id, value);
		}
		return value;
	}

	static merge(events: IMenuRegistryChangeEvent[]): IMenuRegistryChangeEvent {
		const ids = new Set<MenuId>();
		for (const item of events) {
			if (item instanceof MenuRegistryChangeEvent) {
				ids.add(item.id);
			}
		}
		return ids;
	}

	readonly has: (id: MenuId) => boolean;

	private constructor(private readonly id: MenuId) {
		this.has = candidate => candidate === id;
	}
}

export interface IMenuRegistry {
	readonly onDidChangeMenu: Event<IMenuRegistryChangeEvent>;
	addCommand(userCommand: ICommandAction): IDisposable;

	appendMenuItem(menu: MenuId, item: IMenuItem | ISubmenuItem): IDisposable;
	getMenuItems(loc: MenuId): Array<IMenuItem | ISubmenuItem>;
}

export const MenuRegistry: IMenuRegistry = new class implements IMenuRegistry {

	private readonly _commands = new Map<string, ICommandAction>();
	private readonly _menuItems = new Map<MenuId, LinkedList<IMenuItem | ISubmenuItem>>();
	private readonly _onDidChangeMenu = new MicrotaskEmitter<IMenuRegistryChangeEvent>({
		merge: MenuRegistryChangeEvent.merge
	});

	readonly onDidChangeMenu: Event<IMenuRegistryChangeEvent> = this._onDidChangeMenu.event;

	addCommand(command: ICommandAction): IDisposable {
		this._commands.set(command.id, command);
		this._onDidChangeMenu.fire(MenuRegistryChangeEvent.for(MenuId.CommandPalette));

		return markAsSingleton(toDisposable(() => {
			if (this._commands.delete(command.id)) {
				this._onDidChangeMenu.fire(MenuRegistryChangeEvent.for(MenuId.CommandPalette));
			}
		}));
	}

	getCommand(id: string): ICommandAction | undefined {
		return this._commands.get(id);
	}

	getCommands(): ICommandsMap {
		const map = new Map<string, ICommandAction>();
		this._commands.forEach((value, key) => map.set(key, value));
		return map;
	}

	appendMenuItem(id: MenuId, item: IMenuItem | ISubmenuItem): IDisposable {
		let list = this._menuItems.get(id);
		if (!list) {
			list = new LinkedList();
			this._menuItems.set(id, list);
		}
		const rm = list.push(item);
		this._onDidChangeMenu.fire(MenuRegistryChangeEvent.for(id));
		return markAsSingleton(toDisposable(() => {
			rm();
			this._onDidChangeMenu.fire(MenuRegistryChangeEvent.for(id));
		}));
	}

	appendMenuItems(items: Iterable<{ id: MenuId; item: IMenuItem | ISubmenuItem }>): IDisposable {
		const result = new DisposableStore();
		for (const { id, item } of items) {
			result.add(this.appendMenuItem(id, item));
		}
		return result;
	}

	getMenuItems(id: MenuId): Array<IMenuItem | ISubmenuItem> {
		let result: Array<IMenuItem | ISubmenuItem>;
		if (this._menuItems.has(id)) {
			result = [...this._menuItems.get(id)!];
		} else {
			result = [];
		}
		if (id === MenuId.CommandPalette) {
			// CommandPalette is special because it shows
			// all commands by default
			this._appendImplicitItems(result);
		}
		return result;
	}

	private _appendImplicitItems(result: Array<IMenuItem | ISubmenuItem>) {
		const set = new Set<string>();

		for (const item of result) {
			if (isIMenuItem(item)) {
				set.add(item.command.id);
				if (item.alt) {
					set.add(item.alt.id);
				}
			}
		}
		this._commands.forEach((command, id) => {
			if (!set.has(id)) {
				result.push({ command });
			}
		});
	}
};

export class SubmenuItemAction extends SubmenuAction {

	constructor(
		readonly item: ISubmenuItem,
		readonly hideActions: IMenuItemHide | undefined,
		actions: readonly IAction[],
	) {
		super(`submenuitem.${item.submenu.id}`, typeof item.title === 'string' ? item.title : item.title.value, actions, 'submenu');
	}
}

export interface IMenuItemHide {
	readonly isHidden: boolean;
	readonly hide: IAction;
	readonly toggle: IAction;
}

// implements IAction, does NOT extend Action, so that no one
// subscribes to events of Action or modified properties
export class MenuItemAction implements IAction {

	static label(action: ICommandAction, options?: IMenuActionOptions): string {
		return options?.renderShortTitle && action.shortTitle
			? (typeof action.shortTitle === 'string' ? action.shortTitle : action.shortTitle.value)
			: (typeof action.title === 'string' ? action.title : action.title.value);
	}

	readonly item: ICommandAction;
	readonly alt: MenuItemAction | undefined;

	private readonly _options: IMenuActionOptions | undefined;

	readonly id: string;
	readonly label: string;
	readonly tooltip: string;
	readonly class: string | undefined;
	readonly enabled: boolean;
	readonly checked?: boolean;

	constructor(
		item: ICommandAction,
		alt: ICommandAction | undefined,
		options: IMenuActionOptions | undefined,
		readonly hideActions: IMenuItemHide | undefined,
		readonly menuKeybinding: IAction | undefined,
		@IContextKeyService contextKeyService: IContextKeyService,
		@ICommandService private _commandService: ICommandService
	) {
		this.id = item.id;
		this.label = MenuItemAction.label(item, options);
		this.tooltip = (typeof item.tooltip === 'string' ? item.tooltip : item.tooltip?.value) ?? '';
		this.enabled = !item.precondition || contextKeyService.contextMatchesRules(item.precondition);
		this.checked = undefined;

		let icon: ThemeIcon | undefined;

		if (item.toggled) {
			const toggled = ((item.toggled as { condition: ContextKeyExpression }).condition ? item.toggled : { condition: item.toggled }) as {
				condition: ContextKeyExpression; icon?: Icon; tooltip?: string | ILocalizedString; title?: string | ILocalizedString;
			};
			this.checked = contextKeyService.contextMatchesRules(toggled.condition);
			if (this.checked && toggled.tooltip) {
				this.tooltip = typeof toggled.tooltip === 'string' ? toggled.tooltip : toggled.tooltip.value;
			}

			if (this.checked && ThemeIcon.isThemeIcon(toggled.icon)) {
				icon = toggled.icon;
			}

			if (this.checked && toggled.title) {
				this.label = typeof toggled.title === 'string' ? toggled.title : toggled.title.value;
			}
		}

		if (!icon) {
			icon = ThemeIcon.isThemeIcon(item.icon) ? item.icon : undefined;
		}

		this.item = item;
		this.alt = alt ? new MenuItemAction(alt, undefined, options, hideActions, undefined, contextKeyService, _commandService) : undefined;
		this._options = options;
		this.class = icon && ThemeIcon.asClassName(icon);

	}

	run(...args: unknown[]): Promise<void> {
		let runArgs: unknown[] = [];

		if (this._options?.args) {
			runArgs = [...runArgs, ...this._options.args];
		} else if (this._options?.arg) {
			runArgs = [...runArgs, this._options.arg];
		}

		if (this._options?.shouldForwardArgs) {
			runArgs = [...runArgs, ...args];
		}

		return this._commandService.executeCommand(this.id, ...runArgs);
	}
}

//#region --- IAction2

type OneOrN<T> = T | T[];

interface IAction2CommonOptions extends ICommandAction {
	/**
	 * One or many menu items.
	 */
	menu?: OneOrN<{ id: MenuId; precondition?: null } & Omit<IMenuItem, 'command'>>;

	/**
	 * One keybinding.
	 */
	keybinding?: OneOrN<Omit<IKeybindingRule, 'id'>>;
}

interface IBaseAction2Options extends IAction2CommonOptions {

	/**
	 * This type is used when an action is not going to show up in the command palette.
	 * In that case, it's able to use a string for the `title` and `category` properties.
	 */
	f1?: false;
}

export interface ICommandPaletteOptions extends IAction2CommonOptions {

	/**
	 * The title of the command that will be displayed in the command palette after the category.
	 *  This overrides {@link ICommandAction.title} to ensure a string isn't used so that the title
	 *  includes the localized value and the original value for users using language packs.
	 */
	title: ICommandActionTitle;

	/**
	 * Shorthand to add this command to the command palette. Note: this is not the only way to declare that
	 * a command should be in the command palette... however, enforcing ILocalizedString in the other scenarios
	 * is much more challenging and this gets us most of the way there.
	 */
	f1: true;
}

export type IAction2Options = ICommandPaletteOptions | IBaseAction2Options;

export abstract class Action2 {
	constructor(readonly desc: Readonly<IAction2Options>) { }
	abstract run(accessor: ServicesAccessor, ...args: unknown[]): void;
}

export function registerAction2(ctor: { new(): Action2 }): IDisposable {
	const disposables: IDisposable[] = []; // not using `DisposableStore` to reduce startup perf cost
	const action = new ctor();

	const { f1, menu, keybinding, ...command } = action.desc;

	if (CommandsRegistry.getCommand(command.id)) {
		throw new Error(`Cannot register two commands with the same id: ${command.id}`);
	}

	// command
	disposables.push(CommandsRegistry.registerCommand({
		id: command.id,
		handler: (accessor, ...args) => action.run(accessor, ...args),
		metadata: command.metadata ?? { description: action.desc.title }
	}));

	// menu
	if (Array.isArray(menu)) {
		for (const item of menu) {
			disposables.push(MenuRegistry.appendMenuItem(item.id, { command: { ...command, precondition: item.precondition === null ? undefined : command.precondition }, ...item }));
		}

	} else if (menu) {
		disposables.push(MenuRegistry.appendMenuItem(menu.id, { command: { ...command, precondition: menu.precondition === null ? undefined : command.precondition }, ...menu }));
	}
	if (f1) {
		disposables.push(MenuRegistry.appendMenuItem(MenuId.CommandPalette, { command, when: command.precondition }));
		disposables.push(MenuRegistry.addCommand(command));
	}

	// keybinding
	if (Array.isArray(keybinding)) {
		for (const item of keybinding) {
			disposables.push(KeybindingsRegistry.registerKeybindingRule({
				...item,
				id: command.id,
				when: command.precondition ? ContextKeyExpr.and(command.precondition, item.when) : item.when
			}));
		}
	} else if (keybinding) {
		disposables.push(KeybindingsRegistry.registerKeybindingRule({
			...keybinding,
			id: command.id,
			when: command.precondition ? ContextKeyExpr.and(command.precondition, keybinding.when) : keybinding.when
		}));
	}

	return {
		dispose() {
			dispose(disposables);
		}
	};
}
//#endregion
