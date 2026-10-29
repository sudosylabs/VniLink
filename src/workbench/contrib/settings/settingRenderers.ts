import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Emitter, type Event } from '../../../base/common/event.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, type IDisposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { BrandedService, IConstructorSignature } from '../../../platform/instantiation/common/instantiation.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import './media/settingsControls.css';

/**
 * The control for one setting, in the shape of VS Code's setting renderers.
 * The page owns the value: a control reports edits through `onDidChange`, and
 * shows whatever the page last gave it through `setValue`.
 */
export interface ISettingControl extends IDisposable {
	/** Fires with the edited value, which may still be invalid. */
	readonly onDidChange: Event<unknown>;
	/** When true, edits arrive while the user is still typing and are applied after a pause. */
	readonly continuous?: boolean;
	render(container: HTMLElement): void;
	setValue(value: unknown): void;
	setEnabled(enabled: boolean): void;
	/** Shows that the setting is readable but not supported on this computer. */
	setRestricted?(restricted: boolean): void;
	/** Reads the state its choices depend on again, while the page is visible. */
	refresh?(): Promise<void>;
}

/** A setting control's constructor: the setting and its schema, then injected services. */
export type SettingControlCtor = IConstructorSignature<ISettingControl, [key: string, schema: ISettingSchema]>;

export const Extensions = {
	SettingControls: 'workbench.registry.settingControls',
};

/** Controls for settings the generic renderers cannot show, keyed by setting. */
export interface ISettingControlsRegistry {
	// Services is inferred from the constructor, as in VS Code's registerEditorContribution.
	// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
	registerControl<Services extends BrandedService[]>(key: string, ctor: new (key: string, schema: ISettingSchema, ...services: Services) => ISettingControl)
		: void;
	getControl(key: string): SettingControlCtor | undefined;
}

class SettingControlsRegistry implements ISettingControlsRegistry {
	private readonly controls = new Map<string, SettingControlCtor>();

	// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
	public registerControl<Services extends BrandedService[]>(
		key: string,
		ctor: new (key: string, schema: ISettingSchema, ...services: Services) => ISettingControl,
	): void {
		if (this.controls.has(key)) {
			throw new Error(`Setting control for '${key}' is already registered`);
		}
		// As VS Code's editor contributions do: services are resolved by the instantiation service.
		this.controls.set(key, ctor as SettingControlCtor);
	}

	public getControl(key: string): SettingControlCtor | undefined {
		return this.controls.get(key);
	}
}

Registry.add(Extensions.SettingControls, new SettingControlsRegistry());

let nextRowId = 0;

/** The parts of a setting row a control fills in. */
export interface ISettingRow {
	readonly element: HTMLElement;
	/** One line under the title; empty hides it. */
	readonly help: HTMLElement;
	/** Where the control goes, on the right of the row. */
	readonly control: HTMLElement;
}

/**
 * The row every setting uses, as the Connection page lays out its mode: the
 * title and at most one line of help on the left, the control on the right.
 * `target` is the control the title names and the help describes.
 */
export function renderSettingRow(container: HTMLElement, title: string, help: string | undefined, target?: HTMLElement): ISettingRow {
	const id = `setting-row-${String(++nextRowId)}`;
	const element = append(container, $('.setting-row'));
	const text = append(element, target ? $<HTMLLabelElement>('label.setting-row-text') : $('span.setting-row-text'));
	append(text, $('span.setting-row-title')).textContent = title;
	const helpElement = append(text, $('span.setting-row-help', { id: `${id}-help` }));
	helpElement.textContent = help ?? '';
	const control = append(element, $('span.setting-row-control'));
	if (target) {
		target.id ||= id;
		if (text instanceof HTMLLabelElement) {
			text.htmlFor = target.id;
		}
		target.setAttribute('aria-describedby', helpElement.id);
	}
	return { element, help: helpElement, control };
}

/** A switch row: the setting's title and help beside a `role="switch"` checkbox. */
export class SettingBoolControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly input = $<HTMLInputElement>('input.setting-switch', { type: 'checkbox', role: 'switch', disabled: true });
	private help: HTMLElement | undefined;

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, this.schema.description, this.input);
		this.help = row.help;
		this.input.dataset.setting = this.key;
		row.control.append(this.input);
		this._register(addDisposableListener(this.input, EventType.CHANGE, () => {
			if (!this.input.disabled) {
				this.changed.fire(this.input.checked);
			}
		}));
	}

	public setValue(value: unknown): void {
		if (this.input.checked !== (value === true)) {
			this.input.checked = value === true;
		}
	}

	public setEnabled(enabled: boolean): void {
		this.input.disabled = !enabled;
	}

	public setRestricted(restricted: boolean): void {
		const reason = this.schema.restrictedReason ?? nls.localize('settings.unsupported', 'Not supported on this computer.');
		if (this.help) {
			this.help.textContent = restricted ? reason : this.schema.description ?? '';
		}
	}
}

/** A number row with its unit; edits are applied after the user pauses typing. */
export class SettingNumberControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly input = $<HTMLInputElement>('input.setting-number', { type: 'number' });

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, this.schema.description, this.input);
		this.input.min = String(this.schema.minimum ?? '');
		this.input.max = String(this.schema.maximum ?? '');
		this.input.step = this.schema.type === 'integer' ? '1' : 'any';
		this.input.dataset.setting = this.key;
		row.control.append(this.input);
		append(row.control, $('span.setting-unit')).textContent = this.schema.unit ?? '';
		this._register(addDisposableListener(this.input, EventType.INPUT, () => {
			this.changed.fire(this.input.value === '' ? undefined : Number(this.input.value));
		}));
	}

	public setValue(value: unknown): void {
		this.input.value = typeof value === 'number' ? String(value) : '';
	}

	public setEnabled(enabled: boolean): void {
		this.input.disabled = !enabled;
	}
}

/** A native select; the help describes the chosen option when the schema describes each. */
export class SettingEnumControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly select: HTMLSelectElement;
	private help: HTMLElement | undefined;

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
		this.select = createSelect((schema.enum ?? []).map((value, index) => [String(value), schema.enumItemLabels?.[index] ?? String(value)]));
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, undefined, this.select);
		this.help = row.help;
		this.select.dataset.setting = this.key;
		row.control.append(wrapSelect(this.select));
		this._register(addDisposableListener(this.select, EventType.CHANGE, () => {
			this.renderHelp();
			this.changed.fire(this.select.value);
		}));
		this.renderHelp();
	}

	public setValue(value: unknown): void {
		this.select.value = typeof value === 'string' ? value : '';
		this.renderHelp();
	}

	public setEnabled(enabled: boolean): void {
		this.select.disabled = !enabled;
	}

	private renderHelp(): void {
		const index = (this.schema.enum ?? []).indexOf(this.select.value);
		if (this.help) {
			this.help.textContent = this.schema.enumDescriptions?.[index] ?? this.schema.description ?? '';
		}
	}
}

export function createSelect(options: readonly (readonly [string, string])[]): HTMLSelectElement {
	const select = $<HTMLSelectElement>('select');
	for (const [value, label] of options) {
		append(select, $<HTMLOptionElement>('option', { value })).textContent = label;
	}
	return select;
}

/** Keeps the native popup and keyboard behavior while controlling the arrow gutter. */
export function wrapSelect(select: HTMLSelectElement): HTMLElement {
	const wrapper = $('span.settings-select');
	wrapper.append(select, renderIcon(Icons.chevronDown));
	return wrapper;
}
