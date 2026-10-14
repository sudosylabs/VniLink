import { createDecorator } from '../../../../platform/instantiation/common/instantiation.js';
import type { TerminalPolicy } from '../../../../platform/engine/common/engine.js';

export function validTerminalPolicy(policy: TerminalPolicy): boolean {
	return typeof policy.allow_persistence === 'boolean' && [policy.max_sessions, policy.max_sessions_per_peer, policy.history_bytes, policy.replay_bytes, policy.detached_timeout_secs].every(Number.isInteger) && policy.max_sessions >= 1 && policy.max_sessions <= 16 && policy.max_sessions_per_peer >= 1 && policy.max_sessions_per_peer <= policy.max_sessions && policy.history_bytes >= 16384 && policy.history_bytes <= 4194304 && policy.replay_bytes >= 0 && policy.replay_bytes <= policy.history_bytes && policy.detached_timeout_secs >= 1 && policy.detached_timeout_secs <= 86400;
}

export const ITerminalSettingsService = createDecorator<ITerminalSettingsService>('terminalSettingsService');

export interface ITerminalSettingsService {
	readonly _serviceBrand: undefined;
	readonly available: boolean;
	load(): Promise<Readonly<TerminalPolicy>>;
	save(policy: TerminalPolicy): Promise<Readonly<TerminalPolicy>>;
}
