import type { RelayInfo } from '../../../platform/engine/common/engine.js';
import type { ConnectionUpdate } from './connectionService.js';

export function canonicalRelayUrl(url: string): string {
	try {
		return new URL(url.trim()).toString();
	} catch {
		return url.trim();
	}
}

function entries(relays: readonly RelayInfo[]): string[] {
	return relays.map(relay => `${canonicalRelayUrl(relay.url)}|${String(relay.authenticated)}`).sort();
}
/** Credential replacement is always an edit; existing secret values never enter the renderer. */
export function relayDraftChanged(saved: readonly RelayInfo[], draft: ConnectionUpdate): boolean {
	if (draft.mode === 'public') {
		return saved.length > 0;
	}
	return draft.relays.some(relay => relay.replacementToken.length > 0) || JSON.stringify(entries(saved)) !== JSON.stringify(entries(draft.relays));
}
