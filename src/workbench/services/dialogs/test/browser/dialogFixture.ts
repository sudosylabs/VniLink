// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { MessageBoxOptions, MessageBoxReturnValue } from '../../../../../base/parts/sandbox/common/electronTypes.js';
import { InstantiationService } from '../../../../../platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../../../../platform/instantiation/common/serviceCollection.js';
import { INativeHostService } from '../../../../../platform/native/common/native.js';
import type { IDialogOptions, IDialogResult } from '../../dialogService.js';
import { DialogService } from '../../dialogServiceImpl.js';

/** The restart confirmation the Devices page asks before restarting a computer. */
export const restartQuestion: IDialogOptions = {
	type: 'warning',
	message: 'Restart or shut down Studio Mac?',
	detail: 'Its user has 30 seconds to cancel, and it is cancelled if you disconnect before then. Anything not saved there may be lost.',
	buttons: ['Restart', 'Shut Down', 'Cancel'],
	cancelId: 2,
};

/** The question Files asks when a copied file is already there, with a checkbox. */
export const conflictQuestion: IDialogOptions = {
	type: 'warning',
	message: 'Replace notes.txt on Studio Mac?',
	buttons: ['Replace', 'Keep Both', 'Skip'],
	cancelId: 2,
	checkbox: { label: 'Do the same for the other conflicts' },
};

/** The main process's message box: it keeps each box it is asked for and answers with `response`. */
export class TestMessageBoxHost implements Pick<INativeHostService, 'showMessageBox'> {
	declare public readonly _serviceBrand: undefined;
	public readonly shown: MessageBoxOptions[] = [];

	constructor(private readonly response = 0, private readonly checkboxChecked = false) { }

	public showMessageBox(options: MessageBoxOptions): Promise<MessageBoxReturnValue> {
		this.shown.push(options);
		return Promise.resolve({ response: this.response, checkboxChecked: this.checkboxChecked });
	}
}

/** The service as macOS has it: the native box. */
export class NativeDialogService extends DialogService {
	protected override readonly native = true;
}

/** The service as Windows and Linux have it: the dialog drawn in the app's theme. */
export class ThemedDialogService extends DialogService {
	protected override readonly native = false;
}

/** A dialog service of `kind` whose message boxes go to `host`. */
export function createDialogService(kind: typeof NativeDialogService | typeof ThemedDialogService, host: TestMessageBoxHost): DialogService {
	const instantiationService = new InstantiationService(new ServiceCollection([INativeHostService, host]), true);
	const service = instantiationService.createInstance(kind);
	instantiationService.dispose();
	return service;
}

/** Opens the restart confirmation as Windows and Linux draw it; Escape answers it. */
export function showThemedDialog(): Promise<IDialogResult> {
	return createDialogService(ThemedDialogService, new TestMessageBoxHost()).show(restartQuestion);
}
