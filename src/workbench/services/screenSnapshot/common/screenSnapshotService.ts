import { createDecorator } from '../../../../platform/instantiation/common/instantiation.js';
import type { ScreenAccess, ScreenSnapshotResult } from './screenSnapshot.js';

export const IScreenSnapshotService = createDecorator<IScreenSnapshotService>('screenSnapshotService');

/** One remote screenshot for the selected device. This service does not connect or open Desktop. */
export interface IScreenSnapshotService {
	readonly _serviceBrand: undefined;
	capture(endpointId: string, signal: AbortSignal, prompt: boolean): Promise<ScreenSnapshotResult>;
	/** Whether the current connection would still admit a screenshot. This does not capture. */
	access(endpointId: string, signal: AbortSignal): Promise<ScreenAccess>;
}
