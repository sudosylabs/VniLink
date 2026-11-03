/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import * as dom from '../../dom.js';
import * as cssJs from '../../cssValue.js';
import { DomEmitter } from '../../event.js';
import { renderFormattedText, renderText } from '../../formattedTextRenderer.js';
import { ActionBar, IActionViewItemProvider } from '../actionbar/actionbar.js';
import * as aria from '../aria/aria.js';
import { AnchorAlignment, IContextViewProvider } from '../contextview/contextview.js';
import { getBaseLayerHoverDelegate } from '../hover/hoverDelegate2.js';
import { ScrollableElement } from '../scrollbar/scrollableElement.js';
import { Widget } from '../widget.js';
import { IAction } from '../../../common/actions.js';
import { Emitter, Event } from '../../../common/event.js';
import { equals } from '../../../common/objects.js';
import { ScrollbarVisibility } from '../../../common/scrollable.js';
import './inputBox.css';
import * as nls from '../../../../nls.js';
import { DisposableStore, MutableDisposable, type IDisposable } from '../../../common/lifecycle.js';


const $ = dom.$;

export interface IInputOptions {
	readonly placeholder?: string;
	readonly showPlaceholderOnFocus?: boolean;
	readonly tooltip?: string;
	readonly ariaLabel?: string;
	readonly type?: string;
	readonly validationOptions?: IInputValidationOptions;
	readonly flexibleHeight?: boolean;
	readonly flexibleWidth?: boolean;
	readonly flexibleMaxHeight?: number;
	readonly actions?: ReadonlyArray<IAction>;
	readonly actionViewItemProvider?: IActionViewItemProvider;
	readonly inputBoxStyles: IInputBoxStyles;
	readonly hideHoverOnValueChange?: boolean;
}

export interface IInputBoxStyles {
	readonly inputBackground: string | undefined;
	readonly inputForeground: string | undefined;
	readonly inputBorder: string | undefined;
	readonly inputValidationInfoBorder: string | undefined;
	readonly inputValidationInfoBackground: string | undefined;
	readonly inputValidationInfoForeground: string | undefined;
	readonly inputValidationWarningBorder: string | undefined;
	readonly inputValidationWarningBackground: string | undefined;
	readonly inputValidationWarningForeground: string | undefined;
	readonly inputValidationErrorBorder: string | undefined;
	readonly inputValidationErrorBackground: string | undefined;
	readonly inputValidationErrorForeground: string | undefined;
}

export interface IInputValidator {
	(value: string): IMessage | null;
}

export interface IMessage {
	readonly content?: string;
	readonly formatContent?: boolean; // defaults to false
	readonly type?: MessageType;
}

export interface IInputValidationOptions {
	validation?: IInputValidator;
}

export const enum MessageType {
	INFO = 1,
	WARNING = 2,
	ERROR = 3
}

export interface IRange {
	start: number;
	end: number;
}

export class InputBox extends Widget {
	private contextViewProvider?: IContextViewProvider;
	element: HTMLElement;
	protected input: HTMLInputElement;
	private actionbar?: ActionBar;
	private readonly options: IInputOptions;
	private message: IMessage | null;
	protected placeholder: string;
	private tooltip: string;
	private ariaLabel: string;
	private validation?: IInputValidator;
	private state: 'idle' | 'open' | 'closed' = 'idle';

	private mirror: HTMLElement | undefined;
	private cachedHeight: number | undefined;
	private cachedContentHeight: number | undefined;
	private maxHeight: number = Number.POSITIVE_INFINITY;
	private scrollableElement: ScrollableElement | undefined;
	private readonly hover: MutableDisposable<IDisposable> = this._register(new MutableDisposable());
	private readonly messageResizeObserver: MutableDisposable<IDisposable> = this._register(new MutableDisposable());

	private _onDidChange = this._register(new Emitter<string>());
	public get onDidChange(): Event<string> { return this._onDidChange.event; }

	private _onDidHeightChange = this._register(new Emitter<number>());
	public get onDidHeightChange(): Event<number> { return this._onDidHeightChange.event; }

	constructor(container: HTMLElement, contextViewProvider: IContextViewProvider | undefined, options: IInputOptions) {
		super();

		this.contextViewProvider = contextViewProvider;
		this.options = options;

		this.message = null;
		this.placeholder = this.options.placeholder || '';
		this.tooltip = this.options.tooltip ?? (this.placeholder || '');
		this.ariaLabel = this.options.ariaLabel || '';

		if (this.options.validationOptions) {
			this.validation = this.options.validationOptions.validation;
		}

		this.element = dom.append(container, $('.monaco-inputbox.idle'));

		const tagName = this.options.flexibleHeight ? 'textarea' : 'input';

		const wrapper = dom.append(this.element, $('.ibwrapper'));
		this.input = dom.append(wrapper, $(tagName + '.input.empty'));
		this.input.setAttribute('autocorrect', 'off');
		this.input.setAttribute('autocapitalize', 'off');
		this.input.setAttribute('spellcheck', 'false');

		this.onfocus(this.input, () => this.element.classList.add('synthetic-focus'));
		this.onblur(this.input, () => this.element.classList.remove('synthetic-focus'));

		if (this.options.flexibleHeight) {
			this.maxHeight = typeof this.options.flexibleMaxHeight === 'number' ? this.options.flexibleMaxHeight : Number.POSITIVE_INFINITY;

			this.mirror = dom.append(wrapper, $('div.mirror'));
			this.mirror.innerText = '\u00a0';

			this.scrollableElement = new ScrollableElement(this.element, { vertical: ScrollbarVisibility.Auto });

			if (this.options.flexibleWidth) {
				this.input.setAttribute('wrap', 'off');
				this.mirror.style.whiteSpace = 'pre';
				this.mirror.style.wordWrap = 'initial';
			}

			dom.append(container, this.scrollableElement.getDomNode());
			this._register(this.scrollableElement);

			// from ScrollableElement to DOM
			this._register(this.scrollableElement.onScroll(e => this.input.scrollTop = e.scrollTop));

			const onSelectionChange = this._register(new DomEmitter(container.ownerDocument, 'selectionchange'));
			const onAnchoredSelectionChange = Event.filter(onSelectionChange.event, () => {
				const selection = container.ownerDocument.getSelection();
				return selection?.anchorNode === wrapper;
			});

			// from DOM to ScrollableElement
			this._register(onAnchoredSelectionChange(this.updateScrollDimensions, this));
			this._register(this.onDidHeightChange(this.updateScrollDimensions, this));
		} else {
			this.input.type = this.options.type || 'text';
			this.input.setAttribute('wrap', 'off');
		}

		if (this.ariaLabel) {
			this.input.setAttribute('aria-label', this.ariaLabel);
		}

		if (this.placeholder && !this.options.showPlaceholderOnFocus) {
			this.setPlaceHolder(this.placeholder);
		}

		if (this.tooltip) {
			this.setTooltip(this.tooltip);
		}

		this.oninput(this.input, () => this.onValueChange());
		this.onblur(this.input, () => this.onBlur());
		this.onfocus(this.input, () => this.onFocus());

		this._register(this.ignoreGesture(this.input));

		setTimeout(() => this.updateMirror(), 0);

		// Support actions
		if (this.options.actions) {
			this.actionbar = this._register(new ActionBar(this.element, {
				actionViewItemProvider: this.options.actionViewItemProvider
			}));
			this.actionbar.push(this.options.actions, { icon: true, label: false });
		}

		this.applyStyles();
	}

	protected onBlur(): void {
		this._hideMessage();
		if (this.options.showPlaceholderOnFocus) {
			this.input.setAttribute('placeholder', '');
		}
	}

	protected onFocus(): void {
		this._showMessage();
		if (this.options.showPlaceholderOnFocus) {
			this.input.setAttribute('placeholder', this.placeholder || '');
		}
	}

	public setPlaceHolder(placeHolder: string): void {
		this.placeholder = placeHolder;
		this.input.setAttribute('placeholder', placeHolder);
	}

	public setTooltip(tooltip: string): void {
		this.tooltip = tooltip;
		if (!this.hover.value) {
			this.hover.value = this._register(getBaseLayerHoverDelegate().setupDelayedHoverAtMouse(this.input, () => ({
				content: this.tooltip,
				appearance: {
					compact: true,
				}
			})));
		}
	}

	public get inputElement(): HTMLInputElement {
		return this.input;
	}

	public get value(): string {
		return this.input.value;
	}

	public set value(newValue: string) {
		if (this.input.value !== newValue) {
			this.input.value = newValue;
			this.onValueChange();
		}
	}

	public get height(): number {
		return typeof this.cachedHeight === 'number' ? this.cachedHeight : dom.getTotalHeight(this.element);
	}

	public focus(): void {
		this.input.focus();
	}

	public blur(): void {
		this.input.blur();
	}

	public hasFocus(): boolean {
		return dom.isActiveElement(this.input);
	}

	public select(range: IRange | null = null): void {
		this.input.select();

		if (range) {
			this.input.setSelectionRange(range.start, range.end);
			if (range.end === this.input.value.length) {
				this.input.scrollLeft = this.input.scrollWidth;
			}
		}
	}

	public enable(): void {
		this.input.removeAttribute('disabled');
	}

	public disable(): void {
		this.blur();
		this.input.disabled = true;
		this._hideMessage();
	}

	public get width(): number {
		return dom.getTotalWidth(this.input);
	}

	public set width(width: number) {
		if (this.options.flexibleHeight && this.options.flexibleWidth) {
			// textarea with horizontal scrolling
			let horizontalPadding = 0;
			if (this.mirror) {
				const paddingLeft = parseFloat(this.mirror.style.paddingLeft || '') || 0;
				const paddingRight = parseFloat(this.mirror.style.paddingRight || '') || 0;
				horizontalPadding = paddingLeft + paddingRight;
			}
			this.input.style.width = (width - horizontalPadding) + 'px';
		} else {
			this.input.style.width = width + 'px';
		}

		if (this.mirror) {
			this.mirror.style.width = width + 'px';
		}
	}

	private updateScrollDimensions(): void {
		if (typeof this.cachedContentHeight !== 'number' || typeof this.cachedHeight !== 'number' || !this.scrollableElement) {
			return;
		}

		const scrollHeight = this.cachedContentHeight;
		const height = this.cachedHeight;
		const scrollTop = this.input.scrollTop;

		this.scrollableElement.setScrollDimensions({ scrollHeight, height });
		this.scrollableElement.setScrollPosition({ scrollTop });
	}

	public showMessage(message: IMessage, force?: boolean): void {
		if (this.state === 'open' && equals(this.message, message)) {
			// Already showing
			return;
		}

		this.message = message;

		this.element.classList.remove('idle');
		this.element.classList.remove('info');
		this.element.classList.remove('warning');
		this.element.classList.remove('error');
		this.element.classList.add(this.classForType(message.type));

		const styles = this.stylesForType(this.message.type);
		this.element.style.border = `1px solid ${cssJs.asCssValueWithDefault(styles.border, 'transparent')}`;

		if (this.message.content && (this.hasFocus() || force)) {
			this._showMessage();
		}
	}

	public hideMessage(): void {
		this.message = null;

		this.element.classList.remove('info');
		this.element.classList.remove('warning');
		this.element.classList.remove('error');
		this.element.classList.add('idle');

		this._hideMessage();
		this.applyStyles();
	}

	public validate(): MessageType | undefined {
		let errorMsg: IMessage | null = null;

		if (this.validation) {
			errorMsg = this.validation(this.value);

			if (errorMsg) {
				this.inputElement.setAttribute('aria-invalid', 'true');
				this.showMessage(errorMsg);
			}
			else if (this.inputElement.hasAttribute('aria-invalid')) {
				this.inputElement.removeAttribute('aria-invalid');
				this.hideMessage();
			}
		}

		return errorMsg?.type;
	}

	public stylesForType(type: MessageType | undefined): { border: string | undefined; background: string | undefined; foreground: string | undefined } {
		const styles = this.options.inputBoxStyles;
		switch (type) {
			case MessageType.INFO: return { border: styles.inputValidationInfoBorder, background: styles.inputValidationInfoBackground, foreground: styles.inputValidationInfoForeground };
			case MessageType.WARNING: return { border: styles.inputValidationWarningBorder, background: styles.inputValidationWarningBackground, foreground: styles.inputValidationWarningForeground };
			default: return { border: styles.inputValidationErrorBorder, background: styles.inputValidationErrorBackground, foreground: styles.inputValidationErrorForeground };
		}
	}

	private classForType(type: MessageType | undefined): string {
		switch (type) {
			case MessageType.INFO: return 'info';
			case MessageType.WARNING: return 'warning';
			default: return 'error';
		}
	}

	private _showMessage(): void {
		if (!this.contextViewProvider || !this.message) {
			return;
		}

		let div: HTMLElement;
		const layout = () => div.style.width = dom.getTotalWidth(this.element) + 'px';

		this.contextViewProvider.showContextView({
			getAnchor: () => this.element,
			anchorAlignment: AnchorAlignment.RIGHT,
			render: (container: HTMLElement) => {
				if (!this.message) {
					return null;
				}

				div = dom.append(container, $('.monaco-inputbox-container'));
				layout();


				const spanElement = $('span.monaco-inputbox-message');
				if (this.message.formatContent) {
					renderFormattedText(this.message.content!, undefined, spanElement);
				} else {
					renderText(this.message.content!, undefined, spanElement);
				}

				spanElement.classList.add(this.classForType(this.message.type));

				const styles = this.stylesForType(this.message.type);
				spanElement.style.backgroundColor = styles.background ?? '';
				spanElement.style.color = styles.foreground ?? '';
				spanElement.style.border = styles.border ? `1px solid ${styles.border}` : '';

				dom.append(div, spanElement);

				return null;
			},
			onHide: () => {
				this.state = 'closed';
				this.messageResizeObserver.clear();
			},
			layout: layout
		});

		this.observeElementResize();

		// ARIA Support
		let alertText: string;
		if (this.message.type === MessageType.ERROR) {
			alertText = nls.localize('alertErrorMessage', "Error: {0}", this.message.content);
		} else if (this.message.type === MessageType.WARNING) {
			alertText = nls.localize('alertWarningMessage', "Warning: {0}", this.message.content);
		} else {
			alertText = nls.localize('alertInfoMessage', "Info: {0}", this.message.content);
		}

		aria.alert(alertText);

		this.state = 'open';
	}

	private _hideMessage(): void {
		if (!this.contextViewProvider) {
			return;
		}

		if (this.state === 'open') {
			this.contextViewProvider.hideContextView();
		}

		this.messageResizeObserver.clear();
		this.state = 'idle';
	}

	/**
	 * Keeps the validation message sized and anchored to the input while the
	 * message is showing and the input itself is resized, e.g. because the
	 * containing view was resized.
	 */
	private observeElementResize(): void {
		const disposables = new DisposableStore();
		const observer = disposables.add(new dom.DisposableResizeObserver('InputBox.validationMessage', () => {
			// Ignore notifications for a hidden or detached input, laying out
			// against a degenerate anchor would move the message to the corner.
			if (this.element.isConnected && dom.getTotalWidth(this.element) > 0) {
				this.layoutMessage();
			}
		}, dom.getWindow(this.element)));
		disposables.add(observer.observe(this.element));
		this.messageResizeObserver.value = disposables;
	}

	private layoutMessage(): void {
		if (this.state === 'open' && this.contextViewProvider) {
			this.contextViewProvider.layout();
		}
	}

	private onValueChange(): void {
		this._onDidChange.fire(this.value);

		this.validate();
		this.updateMirror();
		this.input.classList.toggle('empty', !this.value);

		if (this.state === 'open' && this.contextViewProvider) {
			this.contextViewProvider.layout();
		}

		if (this.options.hideHoverOnValueChange) {
			getBaseLayerHoverDelegate().hideHover();
		}
	}

	private updateMirror(): void {
		if (!this.mirror) {
			return;
		}

		const value = this.value;
		const lastCharCode = value.charCodeAt(value.length - 1);
		const suffix = lastCharCode === 10 ? ' ' : '';
		const mirrorTextContent = (value + suffix)
			.replace(/\u000c/g, ''); // Don't measure with the form feed character, which messes up sizing

		if (mirrorTextContent) {
			this.mirror.textContent = value + suffix;
		} else {
			this.mirror.innerText = '\u00a0';
		}

		this.layout();
	}

	protected applyStyles(): void {
		const styles = this.options.inputBoxStyles;

		const background = styles.inputBackground ?? '';
		const foreground = styles.inputForeground ?? '';
		const border = styles.inputBorder ?? '';

		this.element.style.backgroundColor = background;
		this.element.style.color = foreground;
		this.input.style.backgroundColor = 'inherit';
		this.input.style.color = foreground;

		// there's always a border, even if the color is not set.
		this.element.style.border = `1px solid ${cssJs.asCssValueWithDefault(border, 'transparent')}`;
	}

	public layout(): void {
		if (!this.mirror) {
			this.layoutMessage();
			return;
		}

		const previousHeight = this.cachedContentHeight;
		this.cachedContentHeight = dom.getTotalHeight(this.mirror);

		if (previousHeight !== this.cachedContentHeight) {
			this.cachedHeight = Math.min(this.cachedContentHeight, this.maxHeight);
			this.input.style.height = this.cachedHeight + 'px';
			this._onDidHeightChange.fire(this.cachedContentHeight);
		}

		this.layoutMessage();
	}

	public override dispose(): void {
		this._hideMessage();

		this.message = null;

		this.actionbar?.dispose();

		super.dispose();
	}
}
