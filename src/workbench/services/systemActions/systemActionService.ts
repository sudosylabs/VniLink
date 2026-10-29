import type { Event } from '../../../base/common/event.js';
import type { SystemActionKind } from '../../../platform/engine/common/engine.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

export const ISystemActionService = createDecorator<ISystemActionService>('systemActionService');

/** Where a lock, restart or shutdown asked of one computer stands, in words. */
export interface SystemActionProgress {
	readonly action: SystemActionKind;
	/** `scheduled` while a restart or shutdown counts down and can be cancelled. */
	readonly phase: 'asking' | 'scheduled' | 'ended';
	readonly message: string;
}

/**
 * Locks, restarts and shuts down paired computers. A restart or shutdown
 * waits out a notice on that computer, where its user can cancel it; this
 * computer can cancel it too until then.
 */
export interface ISystemActionService {
	readonly _serviceBrand: undefined;
	/** Fires with the device id whose progress changed. */
	readonly onDidChange: Event<string>;
	progress(deviceId: string): SystemActionProgress | undefined;
	run(deviceId: string, action: SystemActionKind): Promise<void>;
	/** Cancels the restart or shutdown counting down on `deviceId`. */
	cancel(deviceId: string): Promise<void>;
}
