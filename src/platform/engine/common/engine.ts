export interface Grants { view: boolean; control: boolean; clipboard: boolean; transfer: boolean; }
export interface Device { endpoint_id: string; hostname: string | null; pet_name: string | null; relays: string[]; direct: string[]; grants: Grants; }
export interface Capabilities {
	desktop_capture: boolean; keyboard: boolean; pointer: boolean; clipboard: boolean; file_transfer: boolean; audio: boolean; monitors: number; h264_encode: boolean; h264_decode: boolean;
}
export interface SessionInfo { id: string; generation: string; role: 'Controller' | 'Host'; phase: string | { Failed: { message: string } }; peer_id: string; }
export interface EngineSnapshot { endpoint_id: string; hosting: boolean; public_relay: boolean; relay_urls: string[]; devices: Device[]; capabilities: Capabilities; incoming: SessionInfo | null; outgoing: SessionInfo | null; }
export type EngineCommand =
	| { type: 'snapshot' }
	| { type: 'createInvitation'; ttlSeconds: number }
	| { type: 'acceptInvitation'; token: string }
	| { type: 'exportPairingFile'; ttlSeconds: number; allowControl: boolean }
	| { type: 'openPairingFile'; bytes: number[] }
	| { type: 'confirmPairing'; previewId: string; digits: string }
	| ({ type: 'authorize'; endpointId: string } & Grants)
	| { type: 'revoke' | 'unpair'; endpointId: string }
	| { type: 'setPeerName'; endpointId: string; name: string };
export type EngineReply =
	| { type: 'snapshot'; snapshot: EngineSnapshot }
	| { type: 'invitation'; token: string }
	| { type: 'pairingFile'; file: { bytes: number[]; check: string; expires_at_ms: string } }
	| { type: 'pairingPreview'; preview: { id: string; endpoint_id: string; check: string; expires_at_ms: string } }
	| { type: 'device'; device: Device }
	| { type: 'done' };
export function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function text(value: unknown): value is string {
	return typeof value === 'string' && value.length <= 64 * 1024;
}
function strings(value: unknown): value is string[] {
	return Array.isArray(value) && value.every((item: unknown) => text(item));
}
function bytes(value: unknown): value is number[] {
	return Array.isArray(value) && value.length <= 64 * 1024 && value.every((item: unknown) => typeof item === 'number' && Number.isInteger(item) && item >= 0 && item <= 255);
}
function decimal(value: unknown): value is string {
	return typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
}
function grants(value: Record<string, unknown>): boolean {
	return ['view', 'control', 'clipboard', 'transfer'].every(key => typeof value[key] === 'boolean');
}
function device(value: unknown): value is Device {
	return record(value) && text(value.endpoint_id) && (value.hostname === null || text(value.hostname)) && (value.pet_name === null || text(value.pet_name)) && strings(value.relays) && strings(value.direct) && record(value.grants) && grants(value.grants);
}
function capabilities(value: unknown): value is Capabilities {
	return record(value) && ['desktop_capture', 'keyboard', 'pointer', 'clipboard', 'file_transfer', 'audio', 'h264_encode', 'h264_decode'].every(key => typeof value[key] === 'boolean') && typeof value.monitors === 'number' && Number.isInteger(value.monitors) && value.monitors >= 0 && value.monitors <= 255;
}
function session(value: unknown): value is SessionInfo | null {
	return value === null || (record(value) && decimal(value.id) && decimal(value.generation) && (value.role === 'Controller' || value.role === 'Host') && text(value.peer_id)
		&& ((typeof value.phase === 'string' && ['Connecting', 'Authenticated', 'Authorized', 'Negotiating', 'Active', 'Reconnecting', 'Stopping', 'Closed'].includes(value.phase)) || (record(value.phase) && record(value.phase.Failed) && text(value.phase.Failed.message))));
}
export function isEngineCommand(value: unknown): value is EngineCommand {
	if (!record(value)) {
		return false;
	}
	let keys: string[];
	let valid = false;
	switch (value.type) {
		case 'snapshot': keys = []; valid = true; break;
		case 'createInvitation': keys = ['ttlSeconds']; valid = Number.isInteger(value.ttlSeconds) && typeof value.ttlSeconds === 'number' && value.ttlSeconds > 0 && value.ttlSeconds <= 86400; break;
		case 'acceptInvitation': keys = ['token']; valid = text(value.token); break;
		case 'exportPairingFile': keys = ['ttlSeconds', 'allowControl']; valid = Number.isInteger(value.ttlSeconds) && typeof value.ttlSeconds === 'number' && value.ttlSeconds > 0 && value.ttlSeconds <= 86400 && typeof value.allowControl === 'boolean'; break;
		case 'openPairingFile': keys = ['bytes']; valid = bytes(value.bytes); break;
		case 'confirmPairing': keys = ['previewId', 'digits']; valid = decimal(value.previewId) && typeof value.digits === 'string' && /^[0-9]{3}$/.test(value.digits); break;
		case 'authorize': keys = ['endpointId', 'view', 'control', 'clipboard', 'transfer']; valid = text(value.endpointId) && grants(value); break;
		case 'revoke': case 'unpair': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'setPeerName': keys = ['endpointId', 'name']; valid = text(value.endpointId) && text(value.name) && value.name.length <= 256; break;
		default: return false;
	}
	return valid && Object.keys(value).length === keys.length + 1 && Object.keys(value).every(key => key === 'type' || keys.includes(key));
}
export function isEngineReply(value: unknown): value is EngineReply {
	if (!record(value)) {
		return false;
	}
	switch (value.type) {
		case 'snapshot': return record(value.snapshot) && text(value.snapshot.endpoint_id) && typeof value.snapshot.hosting === 'boolean' && typeof value.snapshot.public_relay === 'boolean' && strings(value.snapshot.relay_urls) && Array.isArray(value.snapshot.devices) && value.snapshot.devices.every((item: unknown) => device(item)) && capabilities(value.snapshot.capabilities) && session(value.snapshot.incoming) && session(value.snapshot.outgoing);
		case 'invitation': return text(value.token);
		case 'pairingFile': return record(value.file) && bytes(value.file.bytes) && text(value.file.check) && decimal(value.file.expires_at_ms);
		case 'pairingPreview': return record(value.preview) && decimal(value.preview.id) && text(value.preview.endpoint_id) && text(value.preview.check) && decimal(value.preview.expires_at_ms);
		case 'device': return device(value.device);
		case 'done': return true;
		default: return false;
	}
}
