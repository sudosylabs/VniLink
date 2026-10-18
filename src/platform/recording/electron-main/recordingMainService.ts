import { app } from 'electron';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { IConfigurationMainService } from '../../configuration/electron-main/configurationMainService.js';
import type { ConfigurationMainService } from '../../configuration/electron-main/configurationMainService.js';
import type { IRecordingService } from '../common/recording.js';

export const IRecordingMainService = createDecorator<RecordingMainService>('recordingMainService');

/** Resolves recording destinations from the saved recording folder. */
export class RecordingMainService implements Omit<IRecordingService, '_serviceBrand'> {
	declare public readonly _serviceBrand: undefined;

	constructor(@IConfigurationMainService private readonly configuration: ConfigurationMainService) { }

	public getDefaultDirectory(): Promise<string> {
		return Promise.resolve(join(app.getPath('pictures'), 'VniLink'));
	}

	public async createDestination(): Promise<string> {
		const saved = this.configuration.getValue('recording.directory');
		const directory = typeof saved === 'string' && saved ? saved : await this.getDefaultDirectory();
		await mkdir(directory, { recursive: true });
		return join(directory, `VniLink-${randomUUID()}.mkv`);
	}
}
