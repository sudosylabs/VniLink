import { IClipboardService } from '../../../platform/clipboard/common/clipboardService.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { INativeHostService } from '../../../platform/native/common/native.js';

/**
 * The system clipboard through the main process, as VS Code's
 * `NativeClipboardService` (workbench/services/clipboard/electron-browser)
 * reaches it: the window's session refuses web clipboard permissions.
 */
export class NativeClipboardService implements IClipboardService {
	declare public readonly _serviceBrand: undefined;

	constructor(@INativeHostService private readonly nativeHostService: INativeHostService) { }

	public async writeText(text: string, type?: 'selection' | 'clipboard'): Promise<void> {
		return this.nativeHostService.writeClipboardText(text, type);
	}

	public async readText(type?: 'selection' | 'clipboard'): Promise<string> {
		return this.nativeHostService.readClipboardText(type);
	}
}

registerSingleton(IClipboardService, NativeClipboardService, InstantiationType.Delayed);
