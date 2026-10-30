// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, append, h } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import type { IListRenderer, IListVirtualDelegate } from '../../../base/browser/ui/list/list.js';
import { List, type IStyleController } from '../../../base/browser/ui/list/listWidget.js';
import { Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { defaultListStyles } from '../../../platform/theme/browser/defaultStyles.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import { devicePlatform, devicePresence, platformIcon } from './deviceLabels.js';

/**
 * Rows are styled by the feature's stylesheet, so the list needs no generated
 * rules, and no inline `<style>` element the content security policy would refuse.
 */
const cssStyleController: IStyleController = {
	style(): void {
		// Focus, selection and hover come from devices.css.
	},
};

/** Every row has the same height: two lines of text, padding and a hairline divider. */
const ROW_HEIGHT = 52;

class DeviceListDelegate implements IListVirtualDelegate<DeviceSummary> {
	public getHeight(): number {
		return ROW_HEIGHT;
	}

	public getTemplateId(): string {
		return DeviceRenderer.TEMPLATE_ID;
	}
}

interface DeviceTemplate {
	readonly glyph: HTMLElement;
	readonly name: HTMLElement;
	readonly presence: HTMLElement;
	os: DeviceSummary['os'] | undefined;
}

class DeviceRenderer implements IListRenderer<DeviceSummary, DeviceTemplate> {
	public static readonly TEMPLATE_ID = 'device';
	public readonly templateId = DeviceRenderer.TEMPLATE_ID;

	public renderTemplate(container: HTMLElement): DeviceTemplate {
		const row = h('.device-row', [
			h('span.device-glyph@glyph'),
			h('span.device-caption', [h('span.device-name@name'), h('span.device-presence@presence')]),
		]);
		append(container, row.root);
		return { glyph: row.glyph, name: row.name, presence: row.presence, os: undefined };
	}

	public renderElement(device: DeviceSummary, _index: number, template: DeviceTemplate): void {
		template.name.textContent = device.name;
		template.name.title = device.name;
		if (template.os !== device.os) {
			template.os = device.os;
			// The selected row shows the filled glyph; the row's label names the platform.
			const icon = platformIcon(device);
			template.glyph.replaceChildren(renderIcon(icon), renderIcon(icon, { variant: 'filled' }));
		}
		template.presence.textContent = nls.localize('devices.rowStatus', '{0} · {1}', devicePresence(device), devicePlatform(device));
		template.presence.dataset.presence = device.presence;
	}

	public disposeTemplate(): void {
		// Templates own only their DOM, which the list removes.
	}
}

/**
 * Remembered computers as a selectable list. Selecting a computer opens its
 * details; rows update in place, so focus and selection survive refreshes.
 */
export class DeviceList extends Disposable {
	public readonly element: HTMLElement;
	private readonly list: List<DeviceSummary>;
	/** Fires when the user selects a computer by click, Enter or Space. */
	public readonly onDidSelect: Event<DeviceSummary>;

	constructor(panelId: string) {
		super();
		this.element = $('.device-list');
		this.list = this._register(new List('Devices', this.element, new DeviceListDelegate(), [new DeviceRenderer()], {
			...defaultListStyles,
			styleController: () => cssStyleController,
			// Rows lay out their own two-line content.
			setRowLineHeight: false,
			identityProvider: { getId: device => device.id },
			multipleSelectionSupport: false,
			accessibilityProvider: {
				getAriaLabel: device => nls.localize('devices.row', '{0}, {1}, {2}', device.name, devicePresence(device), devicePlatform(device)),
				getWidgetAriaLabel: () => nls.localize('devices.list', 'Remembered computers'),
			},
		}));
		this.list.getHTMLElement().setAttribute('aria-controls', panelId);
		// Only the user's selection opens details; `update` restores selection without a browser event.
		const userSelection = Event.filter(this.list.onDidChangeSelection, event => event.browserEvent !== undefined);
		const selected = Event.map(userSelection, event => event.elements.at(0));
		this.onDidSelect = Event.filter<DeviceSummary, undefined>(selected, (device): device is DeviceSummary => device !== undefined);
	}

	public update(devices: readonly DeviceSummary[], selectedId: string | undefined): void {
		const height = devices.length * ROW_HEIGHT;
		this.list.splice(0, this.list.length, devices);
		// The page scrolls, not the list: it is always as tall as its rows.
		this.element.style.height = `${String(height)}px`;
		this.list.layout(height);
		const selected = devices.findIndex(device => device.id === selectedId);
		this.list.setSelection(selected < 0 ? [] : [selected]);
	}

	/** Moves keyboard focus to `id`'s row, or the first row. */
	public focus(id?: string): void {
		const index = Math.max(0, id === undefined ? 0 : this.indexOf(id));
		if (this.list.length) {
			this.list.setFocus([index]);
			this.list.domFocus();
		}
	}

	private indexOf(id: string): number {
		for (let index = 0; index < this.list.length; index++) {
			if (this.list.element(index).id === id) {
				return index;
			}
		}
		return -1;
	}
}
