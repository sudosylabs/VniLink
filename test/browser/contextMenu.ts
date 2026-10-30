// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Separator, toAction } from '../../src/base/common/actions.js';
import { Event } from '../../src/base/common/event.js';
import { Disposable } from '../../src/base/common/lifecycle.js';
import { IContextMenuService, IContextViewService } from '../../src/platform/contextview/browser/contextView.js';
import { ContextMenuService } from '../../src/platform/contextview/browser/contextMenuService.js';
import { ContextViewService } from '../../src/platform/contextview/browser/contextViewService.js';
import { IContextKeyService } from '../../src/platform/contextkey/common/contextkey.js';
import { IMenuService } from '../../src/platform/actions/common/actions.js';
import { SyncDescriptor } from '../../src/platform/instantiation/common/descriptors.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { ILayoutService } from '../../src/platform/layout/browser/layoutService.js';

function assert(value: boolean, message: string): asserts value {
	if (!value) {
		throw new Error(message);
	}
}

const frame = (): Promise<void> => new Promise(resolve => requestAnimationFrame(() => {
	resolve();
}));

function press(key: string, keyCode: number): void {
	const target = document.activeElement ?? document.body;
	for (const type of ['keydown', 'keyup']) {
		target.dispatchEvent(new KeyboardEvent(type, { key, code: key, keyCode, bubbles: true, cancelable: true }));
	}
}

/** The colour a token resolves to, as computed styles report it. */
function resolved(property: 'color' | 'backgroundColor', token: string): string {
	const probe = document.createElement('div');
	probe.style[property] = `var(${token})`;
	document.body.append(probe);
	const value = getComputedStyle(probe)[property];
	probe.remove();
	return value;
}

/** The context menu service as the workbench registers it, over the window's body. */
export class ContextMenuFixture extends Disposable {
	public readonly runs: string[] = [];
	public readonly hides: boolean[] = [];
	private readonly contextMenu: IContextMenuService;

	constructor() {
		super();
		const container = document.body;
		const services = this._register(new InstantiationService(new ServiceCollection(
			[ILayoutService, {
				_serviceBrand: undefined,
				mainContainer: container,
				activeContainer: container,
				containers: [container],
				getContainer: () => container,
				onDidLayoutContainer: Event.None,
			}],
			[IMenuService, { _serviceBrand: undefined }],
			[IContextKeyService, { _serviceBrand: undefined }],
			[IContextViewService, new SyncDescriptor(ContextViewService)],
			[IContextMenuService, new SyncDescriptor(ContextMenuService)],
		), true));
		this.contextMenu = services.invokeFunction(accessor => accessor.get(IContextMenuService));
	}

	/** Opens a menu with Rename and a checked Show Hidden Files, the first item focused. */
	public show(): void {
		const run = (id: string) => () => {
			this.runs.push(id);
		};
		this.contextMenu.showContextMenu({
			getAnchor: () => ({ x: 40, y: 60 }),
			getActions: () => [
				toAction({ id: 'rename', label: 'Rename', run: run('rename') }),
				new Separator(),
				toAction({ id: 'hidden', label: 'Show Hidden Files', checked: true, run: run('hidden') }),
			],
			autoSelectFirstItem: true,
			onHide: didCancel => {
				this.hides.push(didCancel);
			},
		});
	}
}

/**
 * VS Code's context menu service, as the workbench registers it, opens a
 * themed menu with product icons, runs the chosen action, closes on Escape
 * without running anything, and breaks no CSP rule on the way.
 */
export async function verifyContextMenu(): Promise<void> {
	const violations: string[] = [];
	const onViolation = (event: SecurityPolicyViolationEvent): void => {
		violations.push(`${event.violatedDirective} ${event.blockedURI}`);
	};
	document.addEventListener('securitypolicyviolation', onViolation);
	const fixture = new ContextMenuFixture();
	try {
		fixture.show();
		await frame();
		const menu = document.querySelector<HTMLElement>('.context-view.monaco-menu-container .monaco-menu');
		assert(menu !== null, 'No menu opened');
		const items = [...menu.querySelectorAll<HTMLElement>('.action-menu-item')];
		assert(items.length === 2 && items.map(item => item.querySelector('.action-label')?.textContent).join() === 'Rename,Show Hidden Files', 'The menu does not show its two actions');
		assert(items[0].getAttribute('role') === 'menuitem' && items[1].getAttribute('role') === 'menuitemcheckbox' && items[1].getAttribute('aria-checked') === 'true', 'The menu items are not announced as menu items');
		const separator = menu.querySelector<HTMLElement>('.action-label.separator');
		assert(separator !== null && getComputedStyle(separator).borderBottomStyle === 'solid' && getComputedStyle(separator).borderBottomColor === resolved('color', '--line'), 'The separator draws no line');

		// VniLink's stylesheet lays the menu out and its tokens colour it; the check is a product icon.
		assert(getComputedStyle(items[0]).display === 'flex' && items[0].getBoundingClientRect().height === 24, 'The menu stylesheet is not applied');
		const surface = menu.parentElement;
		assert(surface !== null && getComputedStyle(surface).backgroundColor === resolved('backgroundColor', '--surface-raised'), 'The menu is not drawn on the raised surface');
		assert(getComputedStyle(items[1]).color === resolved('color', '--text'), 'The menu text is not the theme text colour');
		const check = items[1].querySelector('.menu-item-check svg');
		assert(check !== null && check.getBoundingClientRect().width > 0 && items[0].querySelector('.codicon') === null, 'The check mark is not a product icon');

		// The keyboard picks the first item; it is drawn in the accent and runs once.
		assert(document.activeElement === items[0], 'The first item is not focused');
		assert(getComputedStyle(items[0]).backgroundColor === resolved('backgroundColor', '--accent'), 'The focused item is not drawn in the accent');
		press('Enter', 13);
		await frame();
		assert(fixture.runs.join() === 'rename', `Enter ran ${fixture.runs.join() || 'nothing'}`);
		assert(document.querySelector('.monaco-menu') === null && fixture.hides.join() === 'false', 'The menu stayed open after running an action');

		// Escape closes without running anything.
		fixture.show();
		await frame();
		assert(document.querySelector('.monaco-menu') !== null, 'The menu did not open again');
		press('Escape', 27);
		await frame();
		assert(document.querySelector('.monaco-menu') === null && fixture.hides.join() === 'false,true' && fixture.runs.length === 1, 'Escape did not cancel the menu');
		assert(violations.length === 0, `The menu broke the CSP: ${violations.join(', ')}`);
	} finally {
		document.removeEventListener('securitypolicyviolation', onViolation);
		fixture.dispose();
	}
}
