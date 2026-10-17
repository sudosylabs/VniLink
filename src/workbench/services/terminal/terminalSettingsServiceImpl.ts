import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { TerminalPolicy } from '../../../platform/engine/common/engine.js';
import { validTerminalPolicy, ITerminalSettingsService } from './terminalSettingsService.js';

export class TerminalSettingsService implements ITerminalSettingsService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();

	constructor(@IEngineService private readonly engine: IEngineService) { }

	public get available(): boolean {
		return this.engine.available;
	}

	public async load(): Promise<Readonly<TerminalPolicy>> {
		if (!this.available) {
			throw new Error('Terminal engine unavailable');
		}
		const reply = await this.engine.execute({ type: 'terminalPolicy' });
		if (reply.type !== 'terminalPolicy' || !validTerminalPolicy(reply.policy)) {
			throw new Error('Invalid terminal policy reply');
		}
		return Object.freeze({ ...reply.policy });
	}

	public save(policy: TerminalPolicy): Promise<Readonly<TerminalPolicy>> {
		const draft = { ...policy };
		const result = this.pending.then(async () => {
			if (!this.available || !validTerminalPolicy(draft)) {
				throw new Error('Invalid terminal policy');
			}
			const reply = await this.engine.execute({ type: 'setTerminalPolicy', policy: draft });
			if (reply.type !== 'done') {
				throw new Error('Terminal policy refused');
			}
			return this.load();
		});
		this.pending = result.catch(() => undefined);
		return result;
	}
}

registerSingleton(ITerminalSettingsService, TerminalSettingsService, InstantiationType.Delayed);
