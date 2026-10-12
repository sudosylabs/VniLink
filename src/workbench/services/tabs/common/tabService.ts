import type { Event } from '../../../../base/common/event.js';
import type { IDisposable } from '../../../../base/common/lifecycle.js';
import type { IconId } from '../../../../base/common/icons.js';
import { createDecorator } from '../../../../platform/instantiation/common/instantiation.js';

export interface TabAction {
	readonly id: string;
	readonly label: string;
	readonly icon?: IconId;
	readonly enabled?: boolean;
	run(signal: AbortSignal): void | Promise<void>;
}
/** A tab owns its view/session; selection never disposes it. */
export interface WorkbenchTab extends IDisposable {
	readonly id: string;
	readonly label: string;
	readonly icon?: IconId;
	readonly actions: readonly TabAction[];
	readonly onDidChangeActions?: Event<void>;
	canClose?(): boolean | Promise<boolean>;
}
export const ITabService = createDecorator<ITabService>('tabService');
export interface ITabService {
	readonly _serviceBrand: undefined;
	readonly tabs: readonly WorkbenchTab[];
	readonly activeTab: WorkbenchTab | undefined;
	readonly onDidChange: Event<void>;
	open(tab: WorkbenchTab): void;
	select(id: string): void;
	move(id: string, index: number): void;
	close(id: string): Promise<boolean>;
	isActionEnabled(tab: WorkbenchTab, action: TabAction): boolean;
	executeAction(id: string): Promise<void>;
}
