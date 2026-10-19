import './media/dialogs.css';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Dialog } from '../../../base/browser/ui/dialog/dialog.js';
import { Icons } from '../../../base/common/icons.js';
import { isMacintosh } from '../../../base/common/platform.js';
import { mainWindow } from '../../../base/browser/window.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { defaultButtonStyles, defaultCheckboxStyles, defaultDialogStyles, defaultInputBoxStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDialogService, type IDialogOptions } from './dialogService.js';

export class DialogService implements IDialogService {
	declare public readonly _serviceBrand: undefined;

	/** macOS's native box follows its appearance; elsewhere the themed one is drawn. */
	protected readonly native: boolean = isMacintosh;

	constructor(@INativeHostService private readonly nativeHost: INativeHostService) { }

	public async show(options: IDialogOptions): Promise<number> {
		if (this.native) {
			const { response } = await this.nativeHost.showMessageBox({
				type: options.type === 'error' ? 'warning' : options.type,
				message: options.message,
				detail: options.detail,
				buttons: [...options.buttons],
				defaultId: options.cancelId,
				cancelId: options.cancelId,
			});
			return response;
		}
		const dialog = new Dialog(mainWindow.document.body, options.message, [...options.buttons], {
			type: options.type,
			detail: options.detail,
			cancelId: options.cancelId,
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
			[...mainWindow.document.querySelectorAll<HTMLElement>('.monaco-dialog-box .dialog-buttons .monaco-button')].find(button => button.textContent === options.buttons[options.cancelId])?.focus();
			return (await result).button;
		} finally {
			dialog.dispose();
		}
	}
}

registerSingleton(IDialogService, DialogService, InstantiationType.Delayed);
