import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { INativeHostService } from '../../src/platform/native/common/native.js';
import { DialogService } from '../../src/workbench/services/dialogs/dialogServiceImpl.js';
import type { IDialogOptions } from '../../src/workbench/services/dialogs/dialogService.js';

function assert(value: boolean, message: string): asserts value {
	if (!value) {
		throw new Error(message);
	}
}

const restart: IDialogOptions = {
	type: 'warning',
	message: 'Restart or shut down Studio Mac?',
	detail: 'Its user has 30 seconds to cancel. Anything not saved there may be lost.',
	buttons: ['Restart', 'Shut Down', 'Cancel'],
	cancelId: 2,
};

/** A dialog service drawn natively or themed, whose native host records each box and answers with `response`. */
function dialogService(native: boolean, response: number, shown: unknown[]): DialogService {
	const services = new InstantiationService(new ServiceCollection([INativeHostService, {
		_serviceBrand: undefined,
		showMessageBox: (options: unknown) => {
			shown.push(options);
			return Promise.resolve({ response, checkboxChecked: false });
		},
	}]), true);
	return services.createInstance(native ? NativeDialogService : ThemedDialogService);
}

class NativeDialogService extends DialogService {
	protected override readonly native = true;
}

class ThemedDialogService extends DialogService {
	protected override readonly native = false;
}

const frame = (): Promise<void> => new Promise(resolve => requestAnimationFrame(() => {
	resolve();
}));

function key(type: 'keydown' | 'keyup', target: EventTarget): void {
	target.dispatchEvent(new KeyboardEvent(type, { key: 'Escape', code: 'Escape', keyCode: 27, bubbles: true, cancelable: true }));
}

export async function verifyDialogs(): Promise<void> {
	// macOS asks through its native box, Cancel answering Return and Escape.
	const shown: unknown[] = [];
	assert(await dialogService(true, 0, shown).show(restart) === 0, 'The native answer was not returned');
	const native = shown[0] as Record<string, unknown> | undefined;
	assert(native?.defaultId === 2 && native.cancelId === 2 && native.message === restart.message, 'The native box does not default to Cancel');

	// Elsewhere VS Code's dialog is drawn in the app's theme.
	const themed = dialogService(false, 0, shown);
	const escaped = themed.show(restart);
	await frame();
	const box = document.querySelector<HTMLElement>('.monaco-dialog-box');
	assert(box !== null && box.getAttribute('role') === 'dialog' && box.getAttribute('aria-modal') === 'true', 'The themed dialog is not a modal dialog');
	assert(shown.length === 1, 'The themed dialog also opened a native box');
	assert(box.querySelector('#monaco-dialog-message-text')?.textContent === restart.message, 'The question is missing');
	assert(box.querySelector('#monaco-dialog-message-detail')?.textContent === restart.detail, 'The detail is missing');
	const buttons = [...box.querySelectorAll<HTMLElement>('.dialog-buttons .monaco-button')].map(button => button.textContent);
	assert(buttons.length === 3 && restart.buttons.every(label => buttons.includes(label)), `The answers are wrong: ${buttons.join(', ')}`);
	assert(document.activeElement?.textContent === 'Cancel', 'Cancel is not focused');
	assert(box.querySelector('.dialog-message-row svg') !== null, 'The warning icon draws nothing');
	assert(box.querySelector('.dialog-close-action') === null, 'A close button without a glyph shows');
	const surface = getComputedStyle(document.documentElement).getPropertyValue('--surface-raised').trim();
	const probe = document.createElement('div');
	probe.style.backgroundColor = surface;
	document.body.append(probe);
	const expected = getComputedStyle(probe).backgroundColor;
	probe.remove();
	assert(getComputedStyle(box).backgroundColor === expected, `The dialog is not themed: ${getComputedStyle(box).backgroundColor} vs ${expected}`);
	key('keydown', document.activeElement);
	key('keyup', document.activeElement);
	assert(await escaped === 2, 'Escape did not cancel');
	await frame();
	assert(document.querySelector('.monaco-dialog-box') === null, 'The dialog stayed after its answer');

	// A click answers with that button.
	const clicked = themed.show(restart);
	await frame();
	[...document.querySelectorAll<HTMLElement>('.monaco-dialog-box .dialog-buttons .monaco-button')].find(button => button.textContent === 'Shut Down')?.click();
	assert(await clicked === 1, 'A click did not answer with its button');
}

/** Opens the restart confirmation as Windows and Linux draw it, for screenshots; Escape closes it. */
export function showThemedDialog(): Promise<number> {
	return dialogService(false, 0, []).show(restart);
}
