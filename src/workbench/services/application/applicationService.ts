import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { Event } from '../../../base/common/event.js';

export const IApplicationService = createDecorator<IApplicationService>('applicationService');

export interface IApplicationService {
	readonly _serviceBrand: undefined;
	readonly restartRequired: boolean;
	readonly onDidChangeRestartRequired: Event<void>;
	restart(): Promise<void>;
	openRelayDocumentation(): Promise<void>;
}
