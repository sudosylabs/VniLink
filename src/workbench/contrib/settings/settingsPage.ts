// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, append } from '../../../base/browser/dom.js';
import { Delayer } from '../../../base/common/async.js';
import { isCancellationError } from '../../../base/common/errors.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { settingSchema, settingSource, validateSetting } from '../../../platform/configuration/common/configurationSchema.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { IWorkbenchConfigurationService } from '../../services/configuration/configurationService.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import { createPageHeading, type IHomePage } from '../home/homePages.js';
import { Extensions, SettingBoolControl, SettingEnumControl, SettingNumberControl, type ISettingControl, type ISettingControlsRegistry } from './settingRenderers.js';
import { type ISettingsPageDescriptor, type ISettingsSectionDescriptor, type ISettingsWidget } from './settingsLayout.js';
import { SettingsStateObserver } from './settingsStateObserver.js';
import './media/settingsForm.css';

/** How long a number edit waits for the user to stop typing before it is applied. */
const continuousEditDelay = 500;

interface SettingRow {
	readonly key: string;
	readonly control: ISettingControl;
	/** Where the row's errors are reported. */
	readonly status: HTMLElement | undefined;
	readonly delayer: Delayer<void>;
	/** The control shows an edit that is not applied yet, or was refused. */
	dirty: boolean;
	busy: boolean;
	/** Increments with every edit, so only the latest edit's outcome is shown. */
	edit: number;
	error: string | undefined;
}

/**
 * A settings page rendered from its layout. Each control applies its setting
 * as soon as the edit is valid, as VS Code's settings editor does; a refused
 * change is reported in the section's status, and engine-owned settings are
 * disabled with the reason while the engine cannot provide them.
 */
export class SettingsPage extends Disposable implements IHomePage {
	public readonly element: HTMLElement;
	private readonly rows: SettingRow[] = [];
	private readonly widgets: ISettingsWidget[] = [];
	private readonly statuses: HTMLElement[] = [];
	private refreshing = false;

	constructor(
		private readonly descriptor: ISettingsPageDescriptor,
		@IWorkbenchConfigurationService private readonly configurationService: IWorkbenchConfigurationService,
		@IInstantiationService private readonly instantiationService: IInstantiationService,
		@IPermissionService private readonly permissionService: IPermissionService,
	) {
		super();
		this.element = $(`main.${descriptor.className ?? `${descriptor.id}-page`}`, { 'data-settings-page': descriptor.id });
		this.render();
		this.registerListeners();
		this.update();
		// Like the forms it replaces, the page reports itself busy until its first read settles.
		void this.refresh();
	}

	private render(): void {
		const descriptor = this.descriptor;
		this.element.append(createPageHeading(descriptor.title, descriptor.description));
		const content = append(this.element, $(`.${descriptor.contentClass.split(' ').join('.')}`));
		const pageStatus = $(`p.${descriptor.statusClass ?? 'settings-status'}`, { role: 'status' });
		for (const section of descriptor.header ?? []) {
			this.renderSection(content, section, pageStatus);
		}
		const fields = descriptor.fieldsClass ? append(content, $(`.${descriptor.fieldsClass}`)) : content;
		for (const section of descriptor.sections) {
			this.renderSection(fields, section, pageStatus);
		}
		if (descriptor.note) {
			append(content, $('p.settings-help.settings-page-note')).textContent = descriptor.note;
		}
		content.append(pageStatus);
		this.statuses.push(pageStatus);
		for (const section of descriptor.footer ?? []) {
			this.renderSection(content, section, pageStatus);
		}
	}

	private renderSection(container: HTMLElement, section: ISettingsSectionDescriptor, pageStatus: HTMLElement | undefined): void {
		if (section.bare) {
			this.renderContents(container, section, pageStatus);
			return;
		}
		const element = append(container, $(`section${section.className ? `.${section.className}` : ''}`, { 'data-section': section.id }));
		this.renderTitle(element, section);
		this.renderContents(element, section, pageStatus);
	}

	private renderTitle(container: HTMLElement, section: ISettingsSectionDescriptor): void {
		if (section.title !== undefined) {
			append(container, $('h2')).textContent = section.title;
		}
		if (section.description) {
			append(container, $(`p.${this.descriptor.descriptionClass}.settings-section-description`)).textContent = section.description;
		}
	}

	private renderContents(container: HTMLElement, section: ISettingsSectionDescriptor, status: HTMLElement | undefined): void {
		for (const key of section.settings ?? []) {
			const control = this.createControl(key);
			control.render(container);
			const delayer = this._register(new Delayer<void>(continuousEditDelay));
			const row: SettingRow = { key, control, status, delayer, dirty: false, busy: false, edit: 0, error: undefined };
			this.rows.push(row);
			this._register(control.onDidChange(value => {
				this.onDidEdit(row, value);
			}));
		}
		if (section.widget) {
			const widget = this._register(this.instantiationService.createInstance<ISettingsWidget>(section.widget));
			widget.render(container);
			this.widgets.push(widget);
		}
	}

	private createControl(key: string): ISettingControl {
		const schema = settingSchema(key);
		if (!schema) {
			throw new Error(`Unknown setting ${key}`);
		}
		const custom = Registry.as<ISettingControlsRegistry>(Extensions.SettingControls).getControl(key);
		if (custom) {
			return this._register(this.instantiationService.createInstance(custom, key, schema));
		}
		switch (schema.type) {
			case 'boolean': return this._register(new SettingBoolControl(key, schema));
			case 'integer':
			case 'number': return this._register(new SettingNumberControl(key, schema));
			case 'string':
				if (schema.enum) {
					return this._register(new SettingEnumControl(key, schema));
				}
				break;
			case 'array':
			case 'object':
			case 'null':
			case undefined:
				break;
		}
		throw new Error(`No control renders setting ${key}`);
	}

	private registerListeners(): void {
		this._register(this.configurationService.onDidChangeConfiguration(event => {
			if (this.rows.some(row => event.affectsConfiguration(row.key))) {
				this.update();
			}
		}));
		if (this.descriptor.refresh) {
			this._register(new SettingsStateObserver(this.element, this.descriptor.refresh, this.permissionService.onDidFocusWindow, () => this.refresh()));
		}
	}

	private onDidEdit(row: SettingRow, value: unknown): void {
		row.edit++;
		row.dirty = true;
		row.error = validateSetting(row.key, value);
		row.delayer.cancel();
		if (row.error === undefined) {
			if (row.control.continuous) {
				row.delayer.trigger(() => this.apply(row, value)).catch((error: unknown) => {
					if (!isCancellationError(error)) {
						throw error;
					}
				});
			} else {
				void this.apply(row, value);
			}
		}
		this.update();
	}

	private async apply(row: SettingRow, value: unknown): Promise<void> {
		const edit = row.edit;
		row.busy = true;
		this.update();
		try {
			await this.configurationService.updateValue(row.key, value);
			if (edit === row.edit) {
				row.dirty = false;
				row.error = undefined;
			}
		} catch (error) {
			if (edit === row.edit) {
				row.error = error instanceof Error ? error.message : nls.localize('settings.failed', 'Could not save these settings.');
				// A refused switch or choice shows the value that is in effect; a typed value stays for correction.
				row.dirty = row.control.continuous === true;
			}
		} finally {
			row.busy = false;
			if (!this.isDisposed()) {
				this.update();
			}
		}
	}

	/** Reads the engine's settings and the widgets' state again. */
	public async refresh(): Promise<void> {
		if (this.refreshing || this.isDisposed()) {
			return;
		}
		this.refreshing = true;
		this.update();
		try {
			const tasks = [...this.widgets, ...this.rows.map(row => row.control)].map(part => part.refresh?.() ?? Promise.resolve());
			if (this.rows.some(row => settingSource(row.key) !== 'user')) {
				tasks.push(this.configurationService.reloadConfiguration());
			}
			await Promise.allSettled(tasks);
		} finally {
			this.refreshing = false;
			if (!this.isDisposed()) {
				this.update();
			}
		}
	}

	private update(): void {
		for (const row of this.rows) {
			const unavailable = this.configurationService.getUnavailableReason(row.key) !== undefined;
			const restricted = this.configurationService.isRestricted(row.key);
			row.control.setRestricted?.(restricted);
			row.control.setEnabled(!unavailable && !restricted && (row.control.continuous === true || !row.busy));
			if (!row.dirty) {
				row.control.setValue(this.configurationService.getValue(row.key));
			}
		}
		for (const status of this.statuses) {
			const rows = this.rows.filter(row => row.status === status);
			const message = rows.find(row => row.error !== undefined)?.error ?? rows.map(row => this.configurationService.getUnavailableReason(row.key)).find(reason => reason !== undefined) ?? '';
			if (status.textContent !== message) {
				status.textContent = message;
			}
		}
		this.element.setAttribute('aria-busy', String(this.refreshing || this.rows.some(row => row.busy)));
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
