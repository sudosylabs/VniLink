// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { app } from 'electron';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { join, parse } from 'node:path';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { IConfigurationMainService } from '../../configuration/electron-main/configurationMainService.js';
import type { ConfigurationMainService } from '../../configuration/electron-main/configurationMainService.js';
import { isPng } from '../../native/common/native.js';
import { isPictureName, type IRecordingService } from '../common/recording.js';

export const IRecordingMainService = createDecorator<RecordingMainService>('recordingMainService');

/** Resolves recording destinations from the saved recording folder. */
export class RecordingMainService implements Omit<IRecordingService, '_serviceBrand'> {
	declare public readonly _serviceBrand: undefined;

	constructor(@IConfigurationMainService private readonly configuration: ConfigurationMainService) { }

	public getDefaultDirectory(): Promise<string> {
		return Promise.resolve(join(app.getPath('pictures'), 'VniLink'));
	}

	public async createDestination(): Promise<string> {
		return join(await this.directory(), `VniLink-${randomUUID()}.mkv`);
	}

	/** The window's arguments arrive unchecked through the proxy channel. */
	public async savePicture(name: unknown, png: unknown): Promise<void> {
		if (!isPictureName(name) || !isPng(png)) {
			throw new Error('Invalid picture');
		}
		const directory = await this.directory();
		const { name: stem } = parse(name);
		for (let copy = 1; copy <= 100; copy++) {
			try {
				await writeFile(join(directory, copy === 1 ? name : `${stem} (${String(copy)}).png`), png.buffer, { flag: 'wx' });
				return;
			} catch (error) {
				if (!(error instanceof Error && 'code' in error && error.code === 'EEXIST')) {
					throw error;
				}
			}
		}
		throw new Error('No free picture name');
	}

	private async directory(): Promise<string> {
		const saved = this.configuration.getValue('recording.directory');
		const directory = typeof saved === 'string' && saved ? saved : await this.getDefaultDirectory();
		await mkdir(directory, { recursive: true });
		return directory;
	}
}
