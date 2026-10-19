import './media/dialogs.css';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Dialog } from '../../../base/browser/ui/dialog/dialog.js';
import { Icons } from '../../../base/common/icons.js';
import { isMacintosh } from '../../../base/common/platform.js';
import { mainWindow } from '../../../base/browser/window.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { defaultButtonStyles, defaultCheckboxStyles, defaultDialogStyles, defaultInputBoxStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDialogService, type IDialogOptions, type IDialogResult } from './dialogService.js';

export class DialogService implements IDialogService {
	declare public readonly _serviceBrand: undefined;

	/** macOS's native box follows its appearance; elsewhere the themed one is drawn. */
	protected readonly native: boolean = isMacintosh;

	constructor(@INativeHostService private readonly nativeHost: INativeHostService) { }

	public async show(options: IDialogOptions): Promise<IDialogResult> {
		if (this.native) {
			const { response, checkboxChecked } = await this.nativeHost.showMessageBox({
				type: options.type === 'error' ? 'warning' : options.type,
				message: options.message,
				detail: options.detail,
				buttons: [...options.buttons],
				defaultId: options.cancelId,
				cancelId: options.cancelId,
				...options.checkbox && { checkboxLabel: options.checkbox.label, checkboxChecked: options.checkbox.checked ?? false },
			});
			return { button: response, checked: options.checkbox !== undefined && checkboxChecked };
		}
		const dialog = new Dialog(mainWindow.document.body, options.message, [...options.buttons], {
			type: options.type,
			detail: options.detail,
			cancelId: options.cancelId,
			checkboxLabel: options.checkbox?.label,
			checkboxChecked: options.checkbox?.checked,
			// Return must not choose a destructive first answer; Escape answers
			// `cancelId`. VniLink has no codicon font for the close button.
			disableDefaultAction: true,
			disableCloseButton: true,
			buttonStyles: defaultButtonStyles,
			checkboxStyles: defaultCheckboxStyles,
			inputBoxStyles: defaultInputBoxStyles,
			dialogStyles: defaultDialogStyles,
		});
		try {
			const result = dialog.show();
			// VniLink draws its own icons; the widget's icon slot takes one in place of a codicon glyph.
			const slot = mainWindow.document.querySelector('.monaco-dialog-box .dialog-message-row > .codicon');
			slot?.replaceChildren(renderIcon(options.type === 'error' ? Icons.error : options.type === 'info' ? Icons.info : Icons.warning));
			// The checkbox's tick is a codicon too; it shows only while checked (dialogs.css).
			mainWindow.document.querySelector('.monaco-dialog-box .dialog-checkbox-row .monaco-checkbox')?.append(renderIcon(Icons.check));
			[...mainWindow.document.querySelectorAll<HTMLElement>('.monaco-dialog-box .dialog-buttons .monaco-button')].find(button => button.textContent === options.buttons[options.cancelId])?.focus();
			const { button, checkboxChecked } = await result;
			return { button, checked: checkboxChecked === true };
		} finally {
			dialog.dispose();
		}
	}
}

registerSingleton(IDialogService, DialogService, InstantiationType.Delayed);
