import { DataChannelClient } from '../../src/platform/engine/browser/dataChannelClient.js';
/** Exercises the actual renderer/preload/main/utility/native handshake: an
 * unknown session must fail promptly through the transferred port. */
export async function verifyDataChannels(): Promise<boolean> {
	for (const kind of ['video', 'input', 'files', 'terminal'] as const) {
		let accepted = false;
		try {
			const channel = await DataChannelClient.open(window.vnilink, { kind, sessionId: '18446744073709551615', generation: '1', ...(kind === 'terminal' ? { rows: 24, columns: 80 } : {}) }, () => {
				throw new Error('Unknown session delivered data');
			}, () => {
				throw new Error('Startup failure escaped its promise');
			});
			accepted = true; channel.close();
		} catch { /* Unknown sessions must fail without losing the engine. */ }
		if (accepted) {
			throw new Error('Unknown data session was accepted');
		}
	}
	const reply = await window.vnilink.engineCommand({ type: 'snapshot' });
	if (reply.type !== 'snapshot') {
		throw new Error('Data-channel failure killed the engine');
	}
	return true;
}
