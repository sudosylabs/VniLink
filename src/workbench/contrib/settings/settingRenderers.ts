import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Emitter, type Event } from '../../../base/common/event.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, type IDisposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { BrandedService, IConstructorSignature } from '../../../platform/instantiation/common/instantiation.js';
import { Registry } from '../../../platform/registry/common/platform.js';
import { AccessToggle } from './accessToggle.js';
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

/** A switch row: title, description and a `role="switch"` checkbox. */
export class SettingBoolControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly toggle: AccessToggle;

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
		this.toggle = this._register(new AccessToggle(schema.title ?? key, schema.description ?? '', allowed => {
			this.changed.fire(allowed);
		}));
	}

	public render(container: HTMLElement): void {
		this.toggle.input.dataset.setting = this.key;
		container.append(this.toggle.element);
	}

	public setValue(value: unknown): void {
		this.toggle.setValue(value === true);
	}

	public setEnabled(enabled: boolean): void {
		this.toggle.setEnabled(enabled);
	}

	public setRestricted(restricted: boolean): void {
		const description = this.schema.description ?? '';
		this.toggle.setDescription(restricted ? nls.localize('security.unsupported', '{0} Not supported on this computer.', description) : description);
	}
}

/** A number row with its unit; edits are applied after the user pauses typing. */
export class SettingNumberControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly input = $<HTMLInputElement>('input', { type: 'number' });

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = append(container, $('label.settings-control-row'));
		const caption = append(row, $('span'));
		if (this.schema.description) {
			append(caption, $('span.access-toggle-title')).textContent = this.schema.title ?? this.key;
			append(caption, $('span.settings-help')).textContent = this.schema.description;
		} else {
			caption.textContent = this.schema.title ?? this.key;
		}
		const control = append(row, $('span.settings-number'));
		this.input.min = String(this.schema.minimum ?? '');
		this.input.max = String(this.schema.maximum ?? '');
		this.input.step = this.schema.type === 'integer' ? '1' : 'any';
		this.input.dataset.setting = this.key;
		control.append(this.input);
		append(control, $('span')).textContent = this.schema.unit ?? '';
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

/** A native select with the shared arrow gutter, followed by the setting's description. */
export class SettingEnumControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly select: HTMLSelectElement;

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
		this.select = createSelect((schema.enum ?? []).map((value, index) => [String(value), schema.enumItemLabels?.[index] ?? String(value)]));
	}

	public render(container: HTMLElement): void {
		const row = append(container, $('label.settings-control-row'));
		append(row, $('span')).textContent = this.schema.title ?? this.key;
		this.select.dataset.setting = this.key;
		row.append(wrapSelect(this.select));
		if (this.schema.description) {
			append(container, $('p.settings-help.setting-enum-help')).textContent = this.schema.description;
		}
		this._register(addDisposableListener(this.select, EventType.CHANGE, () => {
			this.changed.fire(this.select.value);
		}));
	}

	public setValue(value: unknown): void {
		this.select.value = typeof value === 'string' ? value : '';
	}

	public setEnabled(enabled: boolean): void {
		this.select.disabled = !enabled;
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
