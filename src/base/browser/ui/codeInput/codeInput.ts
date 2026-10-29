import './codeInput.css';
import { $, addDisposableListener, append, EventType } from '../../dom.js';
import { Emitter } from '../../../common/event.js';
import { Disposable } from '../../../common/lifecycle.js';

export interface ICodeInputStyles {
	readonly background: string;
	readonly foreground: string;
	readonly border: string;
	readonly focusBorder: string;
}

export interface ICodeInputOptions extends ICodeInputStyles {
	/** How many digits the code has; one box each. */
	readonly length: number;
	/** Names the whole code for assistive technology. */
	readonly label: string;
	/** Names one box from its 1-based position and the code's length. */
	readonly digitLabel: (position: number, length: number) => string;
}

/**
 * A short numeric code as one box per digit. Typing a digit moves to the next
 * box, Backspace on an empty box returns to the previous one, and a paste or
 * an autofilled code spreads across the boxes from where it lands.
 */
export class CodeInput extends Disposable {
	public readonly element: HTMLElement;
	private readonly digits: HTMLInputElement[] = [];
	private readonly _onDidChange = this._register(new Emitter<string>());
	/** The code as typed so far, on every edit. */
	public readonly onDidChange = this._onDidChange.event;
	private readonly _onDidSubmit = this._register(new Emitter<string>());
	/** Enter in any box, with the code as it stands. */
	public readonly onDidSubmit = this._onDidSubmit.event;

	constructor(container: HTMLElement, private readonly options: ICodeInputOptions) {
		super();
		this.element = append(container, $('.code-input', { role: 'group', 'aria-label': options.label }));
		this.element.style.setProperty('--code-input-background', options.background);
		this.element.style.setProperty('--code-input-foreground', options.foreground);
		this.element.style.setProperty('--code-input-border', options.border);
		this.element.style.setProperty('--code-input-focus-border', options.focusBorder);
		for (let index = 0; index < options.length; index++) {
			const digit = append(this.element, $<HTMLInputElement>('input.code-input-digit', {
				type: 'text',
				inputmode: 'numeric',
				// Only the first box takes a one-time code the system offers; the handler spreads it.
				autocomplete: index === 0 ? 'one-time-code' : 'off',
				spellcheck: 'false',
				'aria-label': options.digitLabel(index + 1, options.length),
			}));
			this.digits.push(digit);
			this.registerDigit(digit, index);
		}
	}

	public get value(): string {
		return this.digits.map(digit => digit.value).join('');
	}

	public set value(value: string) {
		const digits = value.replace(/\D/g, '');
		this.digits.forEach((digit, index) => {
			digit.value = digits.charAt(index);
		});
	}

	public get complete(): boolean {
		return this.digits.every(digit => digit.value !== '');
	}

	public set enabled(enabled: boolean) {
		for (const digit of this.digits) {
			digit.disabled = !enabled;
		}
	}

	/** Focuses the first empty box, or the last when the code is complete. */
	public focus(): void {
		(this.digits.find(digit => !digit.value) ?? this.digits.at(-1))?.focus();
	}

	public clear(): void {
		this.value = '';
	}

	private registerDigit(digit: HTMLInputElement, index: number): void {
		this._register(addDisposableListener(digit, EventType.FOCUS, () => {
			digit.select();
		}));
		this._register(addDisposableListener(digit, 'beforeinput', (event: InputEvent) => {
			// Typing over a filled box replaces its digit rather than growing it; other characters are ignored.
			if (event.inputType === 'insertText' && event.data?.length === 1) {
				event.preventDefault();
				if (/^\d$/.test(event.data)) {
					digit.value = event.data;
					this.moveTo(index + 1);
					this.changed();
				}
			}
		}));
		this._register(addDisposableListener(digit, EventType.INPUT, () => {
			const typed = digit.value.replace(/\D/g, '');
			if (typed.length > 1) {
				this.spread(typed, index);
			} else {
				digit.value = typed;
				if (typed) {
					this.moveTo(index + 1);
				}
			}
			this.changed();
		}));
		this._register(addDisposableListener(digit, EventType.PASTE, (event: ClipboardEvent) => {
			const pasted = event.clipboardData?.getData('text').replace(/\D/g, '') ?? '';
			event.preventDefault();
			if (pasted) {
				// A whole code fills from the start wherever it is pasted.
				this.spread(pasted, pasted.length >= this.options.length ? 0 : index);
				this.changed();
			}
		}));
		this._register(addDisposableListener(digit, EventType.KEY_DOWN, (event: KeyboardEvent) => {
			switch (event.key) {
				case 'Backspace':
					if (!digit.value && index > 0) {
						event.preventDefault();
						this.digits[index - 1].value = '';
						this.moveTo(index - 1);
						this.changed();
					}
					break;
				case 'ArrowLeft':
					event.preventDefault();
					this.moveTo(index - 1);
					break;
				case 'ArrowRight':
					event.preventDefault();
					this.moveTo(index + 1);
					break;
				case 'Home':
					event.preventDefault();
					this.moveTo(0);
					break;
				case 'End':
					event.preventDefault();
					this.moveTo(this.options.length - 1);
					break;
				case 'Enter':
					event.preventDefault();
					this._onDidSubmit.fire(this.value);
					break;
			}
		}));
	}

	private spread(digits: string, from: number): void {
		const count = Math.min(digits.length, this.options.length - from);
		for (let offset = 0; offset < count; offset++) {
			this.digits[from + offset].value = digits.charAt(offset);
		}
		this.moveTo(Math.min(from + count, this.options.length - 1));
	}

	private moveTo(index: number): void {
		this.digits[index]?.focus();
	}

	private changed(): void {
		this._onDidChange.fire(this.value);
	}
}
