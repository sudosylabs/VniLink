/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { Codicon } from '../../../common/codicons.js';
import { Emitter, Event } from '../../../common/event.js';
import { IMarkdownString, isMarkdownString } from '../../../common/htmlContent.js';
import { getCodiconAriaLabel, stripIcons } from '../../../common/iconLabels.js';
import { KeyCode } from '../../../common/keyCodes.js';
import { ThemeIcon } from '../../../common/themables.js';
import { isActiveElement, isHTMLElement } from '../../dom.js';
import { IKeyboardEvent } from '../../keyboardEvent.js';
import { HoverStyle, IHoverLifecycleOptions } from '../hover/hover.js';
import { getBaseLayerHoverDelegate } from '../hover/hoverDelegate2.js';
import { Widget } from '../widget.js';
import './toggle.css';

export interface IToggleOpts extends IToggleStyles {
	readonly actionClassName?: string;
	readonly icon?: ThemeIcon;
	readonly title: string | IMarkdownString | HTMLElement;
	readonly isChecked: boolean;
	readonly notFocusable?: boolean;
	readonly hoverLifecycleOptions?: IHoverLifecycleOptions;
}

export interface IToggleStyles {
	readonly inputActiveOptionBorder: string | undefined;
	readonly inputActiveOptionForeground: string | undefined;
	readonly inputActiveOptionBackground: string | undefined;
}

export interface ICheckboxStyles {
	readonly checkboxBackground: string | undefined;
	readonly checkboxBorder: string | undefined;
	readonly checkboxForeground: string | undefined;
	readonly checkboxDisabledBackground: string | undefined;
	readonly checkboxDisabledForeground: string | undefined;
	readonly size?: number;
	readonly hoverLifecycleOptions?: IHoverLifecycleOptions;
}

export const unthemedToggleStyles = {
	inputActiveOptionBorder: '#007ACC00',
	inputActiveOptionForeground: '#FFFFFF',
	inputActiveOptionBackground: '#0E639C50'
};

export class Toggle extends Widget {

	private readonly _onChange = this._register(new Emitter<boolean>());
	get onChange(): Event<boolean /* via keyboard */> { return this._onChange.event; }

	private readonly _onKeyDown = this._register(new Emitter<IKeyboardEvent>());

	private readonly _opts: IToggleOpts;
	private _title: string | IMarkdownString | HTMLElement;
	private _icon: ThemeIcon | undefined;
	readonly domNode: HTMLElement;

	private _checked: boolean;

	constructor(opts: IToggleOpts) {
		super();

		this._opts = opts;
		this._title = this._opts.title;
		this._checked = this._opts.isChecked;

		const classes = ['monaco-custom-toggle'];
		if (this._opts.icon) {
			this._icon = this._opts.icon;
			classes.push(...ThemeIcon.asClassNameArray(this._icon));
		}
		if (this._opts.actionClassName) {
			classes.push(...this._opts.actionClassName.split(' '));
		}
		if (this._checked) {
			classes.push('checked');
		}

		this.domNode = document.createElement('div');
		this._register(getBaseLayerHoverDelegate().setupDelayedHover(this.domNode, () => ({
			content: !isMarkdownString(this._title) && !isHTMLElement(this._title) ? stripIcons(this._title) : this._title,
			style: HoverStyle.Pointer,
		}), this._opts.hoverLifecycleOptions));
		this.domNode.classList.add(...classes);
		if (!this._opts.notFocusable) {
			this.domNode.tabIndex = 0;
		}
		this.domNode.setAttribute('role', 'checkbox');
		this.domNode.setAttribute('aria-checked', String(this._checked));

		this.setTitle(this._opts.title);
		this.applyStyles();

		this.onclick(this.domNode, (ev) => {
			if (this.enabled) {
				this.checked = !this._checked;
				this._onChange.fire(false);
				ev.preventDefault();
				ev.stopPropagation();
			}
		});

		this._register(this.ignoreGesture(this.domNode));

		this.onkeydown(this.domNode, (keyboardEvent) => {
			if (!this.enabled) {
				return;
			}

			if (keyboardEvent.keyCode === KeyCode.Space || keyboardEvent.keyCode === KeyCode.Enter) {
				this.checked = !this._checked;
				this._onChange.fire(true);
				keyboardEvent.preventDefault();
				keyboardEvent.stopPropagation();
				return;
			}

			this._onKeyDown.fire(keyboardEvent);
		});
	}

	get enabled(): boolean {
		return this.domNode.getAttribute('aria-disabled') !== 'true';
	}

	focus(): void {
		this.domNode.focus();
	}

	get checked(): boolean {
		return this._checked;
	}

	set checked(newIsChecked: boolean) {
		this._checked = newIsChecked;

		this.domNode.setAttribute('aria-checked', String(this._checked));
		this.domNode.classList.toggle('checked', this._checked);

		this.applyStyles();
	}

	setIcon(icon: ThemeIcon | undefined): void {
		if (this._icon) {
			this.domNode.classList.remove(...ThemeIcon.asClassNameArray(this._icon));
		}
		this._icon = icon;
		if (this._icon) {
			this.domNode.classList.add(...ThemeIcon.asClassNameArray(this._icon));
		}
	}

	protected applyStyles(): void {
		if (this.domNode) {
			if (this._checked && this._opts.inputActiveOptionBorder) {
				this.domNode.style.setProperty('--monaco-custom-toggle-border-color', this._opts.inputActiveOptionBorder);
			} else {
				this.domNode.style.removeProperty('--monaco-custom-toggle-border-color');
			}
			this.domNode.style.color = (this._checked && this._opts.inputActiveOptionForeground) || 'inherit';
			this.domNode.style.backgroundColor = (this._checked && this._opts.inputActiveOptionBackground) || '';
		}
	}

	setTitle(newTitle: string | IMarkdownString | HTMLElement): void {
		this._title = newTitle;

		const ariaLabel = typeof newTitle === 'string' ? newTitle : isMarkdownString(newTitle) ? newTitle.value : newTitle.textContent;

		this.domNode.setAttribute('aria-label', getCodiconAriaLabel(ariaLabel));
	}

	set visible(visible: boolean) {
		this.domNode.style.display = visible ? '' : 'none';
	}

	get visible() {
		return this.domNode.style.display !== 'none';
	}
}


abstract class BaseCheckbox extends Widget {
	static readonly CLASS_NAME = 'monaco-checkbox';

	protected readonly _onChange = this._register(new Emitter<boolean>());

	constructor(
		protected readonly checkbox: Toggle,
		readonly domNode: HTMLElement,
		protected readonly styles: ICheckboxStyles
	) {
		super();

		this.applyStyles();
	}

	get enabled(): boolean {
		return this.checkbox.enabled;
	}

	focus(): void {
		this.domNode.focus();
	}

	hasFocus(): boolean {
		return isActiveElement(this.domNode);
	}

	protected applyStyles(enabled = this.enabled): void {
		this.domNode.style.color = (enabled ? this.styles.checkboxForeground : this.styles.checkboxDisabledForeground) || '';
		this.domNode.style.backgroundColor = (enabled ? this.styles.checkboxBackground : this.styles.checkboxDisabledBackground) || '';
		this.domNode.style.borderColor = (enabled ? this.styles.checkboxBorder : this.styles.checkboxDisabledBackground) || '';

		const size = this.styles.size || 18;
		this.domNode.style.width =
			this.domNode.style.height =
			this.domNode.style.fontSize = `${size}px`;
		this.domNode.style.fontSize = `${size - 2}px`;
	}
}

export class Checkbox extends BaseCheckbox {
	constructor(title: string, isChecked: boolean, styles: ICheckboxStyles) {
		const toggle = new Toggle({ title, isChecked, icon: Codicon.check, actionClassName: BaseCheckbox.CLASS_NAME, hoverLifecycleOptions: styles.hoverLifecycleOptions, ...unthemedToggleStyles });
		super(toggle, toggle.domNode, styles);

		this._register(toggle);
		this._register(this.checkbox.onChange(keyboard => {
			this.applyStyles();
			this._onChange.fire(keyboard);
		}));
	}

	get checked(): boolean {
		return this.checkbox.checked;
	}

	set checked(newIsChecked: boolean) {
		this.checkbox.checked = newIsChecked;
		this.applyStyles();
	}

	protected override applyStyles(enabled?: boolean): void {
		if (this.checkbox.checked) {
			this.checkbox.setIcon(Codicon.check);
		} else {
			this.checkbox.setIcon(undefined);
		}
		super.applyStyles(enabled);
	}
}
