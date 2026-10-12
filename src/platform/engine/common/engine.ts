export interface Grants { view: boolean; control: boolean; clipboard: boolean; transfer: boolean; }

export interface Device { endpoint_id: string; hostname: string | null; pet_name: string | null; relays: string[]; direct: string[]; grants: Grants; }

export interface Capabilities {
	desktop_capture: boolean; keyboard: boolean; pointer: boolean; clipboard: boolean; file_transfer: boolean; audio: boolean; monitors: number; h264_encode: boolean; h264_decode: boolean;
}

export interface SessionInfo { id: string; generation: string; role: 'Controller' | 'Host'; phase: string | { Failed: { message: string } }; peer_id: string; }

export interface PointerSettings { sensitivity_percent: number; reverse_wheel: boolean; swap_buttons: boolean; }

export interface DisplayInfo { id: string; origin_x: number; origin_y: number; width: number; height: number; rotation_deg: number; scale_percent: number; revision: string; }

export interface QualityLimits { max_bitrate_kbps: number; max_frame_rate: number; max_width: number; max_height: number; }

export interface VideoCodecCaps {
	encode_baseline: boolean; encode_high: boolean; decode_baseline: boolean; decode_high: boolean;
	max_width: number; max_height: number; hardware: boolean; device: string; chroma444: boolean;
}

export interface RelayInput { url: string; authenticated: boolean; replacementToken: string; }

export interface ServiceSummary {
	id: string; connectionId: string; generation: string; peer_id: string;
	kind: 'Desktop' | 'Files' | 'Terminal' | 'Camera' | 'SystemAudio' | 'Microphone' | 'System';
	phase: string | { Closed: { cause: string } } | { Failed: { message: string } };
}

export type EngineEvent =
	| { type: 'hostingChanged'; hosting: boolean }
	| { type: 'devicePaired' | 'deviceAuthorized' | 'deviceRevoked' | 'deviceUnpaired'; endpoint_id: string }
	| { type: 'controlRequested'; endpoint_id: string; check: string }
	| { type: 'addressesUpdated'; endpoint_id: string }
	| { type: 'peerRenamed'; endpoint_id: string; hostname: string | null }
	| { type: 'sessionChanged'; id: string; generation: string; role: 'Controller' | 'Host'; phase: SessionPhase; peer_id: string }
	| { type: 'serviceChanged' | 'hostedServiceChanged' | 'desktopClipboardReceived'; service: ServiceSummary }
	| { type: 'qualityUpdated'; id: string; quality: QualityLimits }
	| { type: 'inputFailed'; id: string; message: string }
	| { type: 'keyframeRequested' | 'clipboardReceived' | 'clipboardFilesReceived'; id: string }
	| { type: 'topologyChanged'; revision: string }
	| { type: 'hostLocked'; locked: boolean };

export type SessionPhase = string | { Failed: { message: string } };

export interface SessionPath { relay: boolean; selected: boolean; rtt_ms: string; tx_bytes: string; rx_bytes: string; }

export interface VideoFrame { keyframe: boolean; timestamp_us: string; data: string; display_id: string; revision: string; }

export interface RelayInfo { url: string; authenticated: boolean; }

export interface ControlRequestInfo { endpoint_id: string; check: string; }

export interface EngineSnapshot { endpoint_id: string; hosting: boolean; public_relay: boolean; relay_urls: string[]; devices: Device[]; capabilities: Capabilities; incoming: SessionInfo | null; outgoing: SessionInfo | null; }

export type EngineCommand =
	| { type: 'beginConnectServices'; endpointId: string }
	| { type: 'snapshot' }
	| { type: 'endpoint' | 'capabilities' | 'videoCodecs' | 'clipboardOwnerSession' | 'subscribeEvents' | 'subscribeVideo' }
	| { type: 'setVideoCodecs'; codecs: VideoCodecCaps }
	| { type: 'saveCustomRelays'; relays: RelayInput[] }
	| { type: 'peerHostname'; endpointId: string }
	| { type: 'connectFiles' | 'beginConnectFiles'; endpointId: string }
	| { type: 'sessionKeyboardMode' | 'sessionSwapControlAndCommand'; sessionId: string; generation: string }
	| { type: 'setSessionKeyboardMode'; sessionId: string; generation: string; mode: 'Character' | 'Map' | 'Translate' }
	| { type: 'setSessionSwapControlAndCommand'; sessionId: string; generation: string; enabled: boolean }
	| { type: 'controllerSessionPaths'; sessionId: string }
	| { type: 'pollEvents'; subscriptionId: string; maxEvents: number }
	| { type: 'releaseEvents'; subscriptionId: string }
	| { type: 'createInvitation'; ttlSeconds: number }
	| { type: 'acceptInvitation'; token: string }
	| { type: 'exportPairingFile'; ttlSeconds: number; allowControl: boolean }
	| { type: 'openPairingFile'; bytes: number[] }
	| { type: 'openPairingFileBase64'; data: string }
	| { type: 'confirmPairing'; previewId: string; digits: string }
	| ({ type: 'authorize'; endpointId: string } & Grants)
	| { type: 'revoke' | 'unpair'; endpointId: string }
	| { type: 'setPeerName'; endpointId: string; name: string }
	| { type: 'connect' | 'beginConnect'; endpointId: string }
	| { type: 'disconnect' }
	| { type: 'disconnectSession'; sessionId: string }
	| { type: 'submitKey'; sessionId: string; generation: string; usage: number; down: boolean }
	| { type: 'submitPointer'; sessionId: string; generation: string; x: number; y: number; display: string; revision: string }
	| { type: 'submitButton'; sessionId: string; generation: string; button: 'Left' | 'Right' | 'Middle' | 'Back' | 'Forward'; down: boolean }
	| { type: 'submitWheel' | 'submitMotion'; sessionId: string; generation: string; dx: number; dy: number }
	| { type: 'submitPreciseWheel'; sessionId: string; generation: string; dx: number; dy: number }
	| { type: 'submitText' | 'submitClipboard'; sessionId: string; generation: string; text: string }
	| { type: 'submitLocalKey'; sessionId: string; generation: string; usage: number; down: boolean; text: string }
	| { type: 'releaseSessionInput'; sessionId: string; generation: string }
	| { type: 'keyboardMode' }
	| { type: 'setKeyboardMode'; mode: 'Character' | 'Map' | 'Translate' }
	| { type: 'swapControlAndCommand' }
	| { type: 'setSwapControlAndCommand'; enabled: boolean }
	| { type: 'sessionPointerSettings'; sessionId: string; generation: string }
	| { type: 'setSessionPointerSettings'; sessionId: string; generation: string; settings: PointerSettings }
	| { type: 'sessionClipboardText'; sessionId: string; generation: string }
	| { type: 'clipboardText' }
	| { type: 'startHosting' | 'stopHosting' | 'stopHostedSession' | 'resetIdentity' }
	| { type: 'setRelayPublic' }
	| { type: 'setRelayCustom'; urls: string[] }
	| { type: 'savedRelays' }
	| { type: 'controlCheck' | 'controlRequest' | 'rejectControlRequest' }
	| { type: 'confirmControlRequest'; digits: string }
	| { type: 'controllerSessions' | 'hostedSessions' }
	| { type: 'controllerSession'; sessionId: string }
	| { type: 'forgetControllerSession'; sessionId: string }
	| { type: 'sessionPaths'; sessionId: string }
	| { type: 'topology' }
	| { type: 'sessionTopology'; sessionId: string; generation: string }
	| { type: 'selectDisplays'; sessionId: string; generation: string; ids: string[] }
	| { type: 'captureDisplays' }
	| { type: 'setTopology'; revision: string; displays: DisplayInfo[] }
	| { type: 'updateSessionQuality'; sessionId: string; role: 'Controller' | 'Host'; quality: QualityLimits }
	| { type: 'subscribeSessionVideo'; sessionId: string; generation: string }
	| { type: 'pollVideo'; subscriptionId: string }
	| { type: 'releaseVideo'; subscriptionId: string }
	| { type: 'publishVideo'; sessionId: string; keyframe: boolean; timestampUs: string; data: string };

export type EngineReply =
	| { type: 'snapshot'; snapshot: EngineSnapshot }
	| { type: 'invitation'; token: string }
	| { type: 'pairingFile'; file: { bytes: number[]; check: string; expires_at_ms: string } }
	| { type: 'pairingPreview'; preview: { id: string; endpoint_id: string; check: string; expires_at_ms: string } }
	| { type: 'device'; device: Device }
	| { type: 'session'; session: SessionInfo }
	| { type: 'sessions'; sessions: SessionInfo[] }
	| { type: 'text'; text: string | null }
	| { type: 'check'; check: string }
	| { type: 'controlRequest'; request: ControlRequestInfo | null }
	| { type: 'keyboardMode'; mode: 'Character' | 'Map' | 'Translate' }
	| { type: 'swap'; enabled: boolean }
	| { type: 'pointerSettings'; settings: PointerSettings }
	| { type: 'displays'; displays: DisplayInfo[] }
	| { type: 'displayIds'; displays: string[] }
	| { type: 'paths'; paths: SessionPath[] }
	| { type: 'relays'; relays: RelayInfo[] }
	| { type: 'videoSubscription'; subscriptionId: string }
	| { type: 'videoFrame'; frame: VideoFrame | null }
	| { type: 'endpoint'; endpointId: string }
	| { type: 'capabilities'; capabilities: Capabilities }
	| { type: 'videoCodecs'; codecs: VideoCodecCaps }
	| { type: 'hostname'; hostname: string | null }
	| { type: 'ownerSession'; session: SessionInfo | null }
	| { type: 'eventSubscription'; subscriptionId: string }
	| { type: 'events'; events: EngineEvent[]; lagged: string }
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

function base64(value: unknown): value is string {
	return typeof value === 'string' && value.length > 0 && value.length <= 32768 && value.length % 4 === 0 && /^[A-Za-z0-9+/]*={0,2}$/.test(value);
}

function longText(value: unknown): value is string {
	return typeof value === 'string' && value.length <= 256 * 1024;
}

function frameData(value: unknown): value is string {
	return typeof value === 'string' && value.length > 0 && value.length <= 1024 * 1024 && value.length % 4 === 0 && /^[A-Za-z0-9+/]*={0,2}$/.test(value);
}

function uint(value: unknown, max: number): boolean {
	return typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= max;
}

function int(value: unknown, min: number, max: number): boolean {
	return typeof value === 'number' && Number.isInteger(value) && value >= min && value <= max;
}

function decimals(value: unknown): value is string[] {
	return Array.isArray(value) && value.length <= 16 && value.every((item: unknown) => decimal(item));
}

function keyboardMode(value: unknown): value is 'Character' | 'Map' | 'Translate' {
	return value === 'Character' || value === 'Map' || value === 'Translate';
}

function pointerButton(value: unknown): value is 'Left' | 'Right' | 'Middle' | 'Back' | 'Forward' {
	return value === 'Left' || value === 'Right' || value === 'Middle' || value === 'Back' || value === 'Forward';
}

function sessionRole(value: unknown): value is 'Controller' | 'Host' {
	return value === 'Controller' || value === 'Host';
}

function pointerSettings(value: unknown): value is PointerSettings {
	return record(value) && uint(value.sensitivity_percent, 65535) && typeof value.reverse_wheel === 'boolean' && typeof value.swap_buttons === 'boolean' && Object.keys(value).length === 3;
}

function display(value: unknown): value is DisplayInfo {
	return record(value) && decimal(value.id) && int(value.origin_x, -32768, 32767) && int(value.origin_y, -32768, 32767)
		&& uint(value.width, 65535) && uint(value.height, 65535) && uint(value.rotation_deg, 360) && uint(value.scale_percent, 800) && decimal(value.revision) && Object.keys(value).length === 8;
}

function displays(value: unknown): value is DisplayInfo[] {
	return Array.isArray(value) && value.length <= 16 && value.every((item: unknown) => display(item));
}

function quality(value: unknown): value is QualityLimits {
	return record(value) && uint(value.max_bitrate_kbps, 4294967295) && uint(value.max_frame_rate, 255) && uint(value.max_width, 65535)
		&& uint(value.max_height, 65535) && Object.keys(value).length === 4;
}

function videoCodecs(value: unknown): value is VideoCodecCaps {
	return record(value) && typeof value.encode_baseline === 'boolean' && typeof value.encode_high === 'boolean'
		&& typeof value.decode_baseline === 'boolean' && typeof value.decode_high === 'boolean'
		&& uint(value.max_width, 65535) && uint(value.max_height, 65535) && typeof value.hardware === 'boolean'
		&& text(value.device) && typeof value.chroma444 === 'boolean' && Object.keys(value).length === 9;
}

function relayInput(value: unknown): value is RelayInput {
	return record(value) && text(value.url) && typeof value.authenticated === 'boolean'
		&& typeof value.replacementToken === 'string' && value.replacementToken.length <= 64 * 1024
		&& Object.keys(value).length === 3;
}

function relayInputs(value: unknown): value is RelayInput[] {
	return Array.isArray(value) && value.length <= 16 && value.every((item: unknown) => relayInput(item));
}

function serviceKind(value: unknown): value is ServiceSummary['kind'] {
	return value === 'Desktop' || value === 'Files' || value === 'Terminal' || value === 'Camera'
		|| value === 'SystemAudio' || value === 'Microphone' || value === 'System';
}

function serviceCause(value: unknown): value is string {
	return value === 'OwnerClose' || value === 'ParentLoss' || value === 'Revoked' || value === 'Replaced'
		|| value === 'DeviceLoss' || value === 'Shutdown' || value === 'PeerClose' || value === 'IdleExpired';
}

function servicePhase(value: unknown): value is ServiceSummary['phase'] {
	if (typeof value === 'string') {
		return value === 'Waiting' || value === 'Active' || value === 'Suspended';
	}
	if (!record(value) || Object.keys(value).length !== 1) {
		return false;
	}
	if ('Closed' in value) {
		return record(value.Closed) && serviceCause(value.Closed.cause) && Object.keys(value.Closed).length === 1;
	}
	if ('Failed' in value) {
		return record(value.Failed) && text(value.Failed.message) && Object.keys(value.Failed).length === 1;
	}
	return false;
}

function serviceInfo(value: unknown): value is ServiceSummary {
	return record(value) && decimal(value.id) && decimal(value.connectionId) && decimal(value.generation)
		&& text(value.peer_id) && serviceKind(value.kind) && servicePhase(value.phase)
		&& Object.keys(value).length === 6;
}

function sessionPhase(value: unknown): value is SessionPhase {
	return (typeof value === 'string' && ['Connecting', 'Authenticated', 'Authorized', 'Negotiating', 'Active', 'Reconnecting', 'Stopping', 'Closed'].includes(value))
		|| (record(value) && record(value.Failed) && text(value.Failed.message));
}

export function isEngineEvent(value: unknown): value is EngineEvent {
	if (!record(value) || typeof value.type !== 'string') {
		return false;
	}
	switch (value.type) {
		case 'hostingChanged': return typeof value.hosting === 'boolean' && Object.keys(value).length === 2;
		case 'devicePaired': case 'deviceAuthorized': case 'deviceRevoked': case 'deviceUnpaired':
			return text(value.endpoint_id) && Object.keys(value).length === 2;
		case 'controlRequested': return text(value.endpoint_id) && text(value.check) && Object.keys(value).length === 3;
		case 'addressesUpdated': return text(value.endpoint_id) && Object.keys(value).length === 2;
		case 'peerRenamed': return text(value.endpoint_id) && (value.hostname === null || text(value.hostname)) && Object.keys(value).length === 3;
		case 'sessionChanged':
			return decimal(value.id) && decimal(value.generation) && sessionRole(value.role) && sessionPhase(value.phase)
				&& text(value.peer_id) && Object.keys(value).length === 6;
		case 'serviceChanged': case 'hostedServiceChanged': case 'desktopClipboardReceived':
			return serviceInfo(value.service) && Object.keys(value).length === 2;
		case 'qualityUpdated': return decimal(value.id) && quality(value.quality) && Object.keys(value).length === 3;
		case 'inputFailed': return decimal(value.id) && text(value.message) && Object.keys(value).length === 3;
		case 'keyframeRequested': case 'clipboardReceived': case 'clipboardFilesReceived':
			return decimal(value.id) && Object.keys(value).length === 2;
		case 'topologyChanged': return decimal(value.revision) && Object.keys(value).length === 2;
		case 'hostLocked': return typeof value.locked === 'boolean' && Object.keys(value).length === 2;
		default: return false;
	}
}

function sessionPath(value: unknown): value is SessionPath {
	return record(value) && typeof value.relay === 'boolean' && typeof value.selected === 'boolean' && decimal(value.rtt_ms)
		&& decimal(value.tx_bytes) && decimal(value.rx_bytes) && Object.keys(value).length === 5;
}

function videoFrame(value: unknown): value is VideoFrame {
	return record(value) && typeof value.keyframe === 'boolean' && decimal(value.timestamp_us) && frameData(value.data)
		&& decimal(value.display_id) && decimal(value.revision) && Object.keys(value).length === 5;
}

function relay(value: unknown): value is RelayInfo {
	return record(value) && text(value.url) && typeof value.authenticated === 'boolean' && Object.keys(value).length === 2;
}

function controlRequest(value: unknown): value is ControlRequestInfo | null {
	return value === null || (record(value) && text(value.endpoint_id) && text(value.check) && Object.keys(value).length === 2);
}

function sessions(value: unknown): value is SessionInfo[] {
	return Array.isArray(value) && value.length <= 64 && value.every((item: unknown) => item !== null && session(item));
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
		case 'beginConnectServices': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'snapshot': keys = []; valid = true; break;
		case 'endpoint': case 'capabilities': case 'videoCodecs': case 'clipboardOwnerSession':
		case 'subscribeEvents': case 'subscribeVideo': keys = []; valid = true; break;
		case 'setVideoCodecs': keys = ['codecs']; valid = videoCodecs(value.codecs); break;
		case 'saveCustomRelays': keys = ['relays']; valid = relayInputs(value.relays); break;
		case 'peerHostname': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'createInvitation': keys = ['ttlSeconds']; valid = Number.isInteger(value.ttlSeconds) && typeof value.ttlSeconds === 'number' && value.ttlSeconds > 0 && value.ttlSeconds <= 86400; break;
		case 'acceptInvitation': keys = ['token']; valid = text(value.token); break;
		case 'exportPairingFile': keys = ['ttlSeconds', 'allowControl']; valid = Number.isInteger(value.ttlSeconds) && typeof value.ttlSeconds === 'number' && value.ttlSeconds > 0 && value.ttlSeconds <= 86400 && typeof value.allowControl === 'boolean'; break;
		case 'openPairingFile': keys = ['bytes']; valid = bytes(value.bytes); break;
		case 'openPairingFileBase64': keys = ['data']; valid = base64(value.data); break;
		case 'confirmPairing': keys = ['previewId', 'digits']; valid = decimal(value.previewId) && typeof value.digits === 'string' && /^[0-9]{3}$/.test(value.digits); break;
		case 'authorize': keys = ['endpointId', 'view', 'control', 'clipboard', 'transfer']; valid = text(value.endpointId) && grants(value); break;
		case 'revoke': case 'unpair': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'setPeerName': keys = ['endpointId', 'name']; valid = text(value.endpointId) && text(value.name) && value.name.length <= 256; break;
		case 'connect': case 'beginConnect': case 'connectFiles': case 'beginConnectFiles': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'disconnect': keys = []; valid = true; break;
		case 'disconnectSession': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'submitKey': keys = ['sessionId', 'generation', 'usage', 'down']; valid = decimal(value.sessionId) && decimal(value.generation) && uint(value.usage, 65535) && typeof value.down === 'boolean'; break;
		case 'submitPointer': keys = ['sessionId', 'generation', 'x', 'y', 'display', 'revision']; valid = decimal(value.sessionId) && decimal(value.generation) && int(value.x, -2147483648, 2147483647) && int(value.y, -2147483648, 2147483647) && decimal(value.display) && decimal(value.revision); break;
		case 'submitButton': keys = ['sessionId', 'generation', 'button', 'down']; valid = decimal(value.sessionId) && decimal(value.generation) && pointerButton(value.button) && typeof value.down === 'boolean'; break;
		case 'submitWheel': case 'submitMotion': keys = ['sessionId', 'generation', 'dx', 'dy']; valid = decimal(value.sessionId) && decimal(value.generation) && int(value.dx, -32768, 32767) && int(value.dy, -32768, 32767); break;
		case 'submitPreciseWheel': keys = ['sessionId', 'generation', 'dx', 'dy']; valid = decimal(value.sessionId) && decimal(value.generation) && int(value.dx, -2147483648, 2147483647) && int(value.dy, -2147483648, 2147483647); break;
		case 'submitText': case 'submitClipboard': keys = ['sessionId', 'generation', 'text']; valid = decimal(value.sessionId) && decimal(value.generation) && longText(value.text); break;
		case 'submitLocalKey': keys = ['sessionId', 'generation', 'usage', 'down', 'text']; valid = decimal(value.sessionId) && decimal(value.generation) && uint(value.usage, 65535) && typeof value.down === 'boolean' && text(value.text); break;
		case 'releaseSessionInput': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'keyboardMode': keys = []; valid = true; break;
		case 'setKeyboardMode': keys = ['mode']; valid = keyboardMode(value.mode); break;
		case 'sessionKeyboardMode': case 'sessionSwapControlAndCommand': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'setSessionKeyboardMode': keys = ['sessionId', 'generation', 'mode']; valid = decimal(value.sessionId) && decimal(value.generation) && keyboardMode(value.mode); break;
		case 'setSessionSwapControlAndCommand': keys = ['sessionId', 'generation', 'enabled']; valid = decimal(value.sessionId) && decimal(value.generation) && typeof value.enabled === 'boolean'; break;
		case 'swapControlAndCommand': keys = []; valid = true; break;
		case 'setSwapControlAndCommand': keys = ['enabled']; valid = typeof value.enabled === 'boolean'; break;
		case 'sessionPointerSettings': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'setSessionPointerSettings': keys = ['sessionId', 'generation', 'settings']; valid = decimal(value.sessionId) && decimal(value.generation) && pointerSettings(value.settings); break;
		case 'sessionClipboardText': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'clipboardText': keys = []; valid = true; break;
		case 'startHosting': case 'stopHosting': case 'stopHostedSession': case 'resetIdentity': keys = []; valid = true; break;
		case 'setRelayPublic': keys = []; valid = true; break;
		case 'setRelayCustom': keys = ['urls']; valid = strings(value.urls); break;
		case 'savedRelays': keys = []; valid = true; break;
		case 'controlCheck': case 'controlRequest': case 'rejectControlRequest': keys = []; valid = true; break;
		case 'confirmControlRequest': keys = ['digits']; valid = typeof value.digits === 'string' && /^[0-9]{3}$/.test(value.digits); break;
		case 'controllerSessions': case 'hostedSessions': keys = []; valid = true; break;
		case 'controllerSession': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'forgetControllerSession': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'sessionPaths': case 'controllerSessionPaths': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'pollEvents': keys = ['subscriptionId', 'maxEvents']; valid = decimal(value.subscriptionId) && typeof value.maxEvents === 'number' && Number.isInteger(value.maxEvents) && value.maxEvents >= 1 && value.maxEvents <= 256; break;
		case 'releaseEvents': keys = ['subscriptionId']; valid = decimal(value.subscriptionId); break;
		case 'topology': keys = []; valid = true; break;
		case 'sessionTopology': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'selectDisplays': keys = ['sessionId', 'generation', 'ids']; valid = decimal(value.sessionId) && decimal(value.generation) && decimals(value.ids); break;
		case 'captureDisplays': keys = []; valid = true; break;
		case 'setTopology': keys = ['revision', 'displays']; valid = decimal(value.revision) && displays(value.displays); break;
		case 'updateSessionQuality': keys = ['sessionId', 'role', 'quality']; valid = decimal(value.sessionId) && sessionRole(value.role) && quality(value.quality); break;
		case 'subscribeSessionVideo': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'pollVideo': case 'releaseVideo': keys = ['subscriptionId']; valid = decimal(value.subscriptionId); break;
		case 'publishVideo': keys = ['sessionId', 'keyframe', 'timestampUs', 'data']; valid = decimal(value.sessionId) && typeof value.keyframe === 'boolean' && decimal(value.timestampUs) && frameData(value.data); break;
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
		case 'session': return value.session !== null && session(value.session);
		case 'sessions': return Array.isArray(value.sessions) && sessions(value.sessions) && Object.keys(value).length === 2;
		case 'text': return (value.text === null || longText(value.text)) && Object.keys(value).length === 2;
		case 'check': return text(value.check) && Object.keys(value).length === 2;
		case 'controlRequest': return controlRequest(value.request) && Object.keys(value).length === 2;
		case 'keyboardMode': return keyboardMode(value.mode) && Object.keys(value).length === 2;
		case 'swap': return typeof value.enabled === 'boolean' && Object.keys(value).length === 2;
		case 'pointerSettings': return pointerSettings(value.settings) && Object.keys(value).length === 2;
		case 'displays': return displays(value.displays) && Object.keys(value).length === 2;
		case 'displayIds': return decimals(value.displays) && Object.keys(value).length === 2;
		case 'paths': return Array.isArray(value.paths) && value.paths.every((item: unknown) => sessionPath(item)) && Object.keys(value).length === 2;
		case 'relays': return Array.isArray(value.relays) && value.relays.every((item: unknown) => relay(item)) && Object.keys(value).length === 2;
		case 'videoSubscription': return decimal(value.subscriptionId) && Object.keys(value).length === 2;
		case 'videoFrame': return (value.frame === null || videoFrame(value.frame)) && Object.keys(value).length === 2;
		case 'endpoint': return text(value.endpointId) && Object.keys(value).length === 2;
		case 'capabilities': return capabilities(value.capabilities) && Object.keys(value).length === 2;
		case 'videoCodecs': return videoCodecs(value.codecs) && Object.keys(value).length === 2;
		case 'hostname': return (value.hostname === null || text(value.hostname)) && Object.keys(value).length === 2;
		case 'ownerSession': return session(value.session) && Object.keys(value).length === 2;
		case 'eventSubscription': return decimal(value.subscriptionId) && Object.keys(value).length === 2;
		case 'events': return Array.isArray(value.events) && value.events.length <= 256 && value.events.every((item: unknown) => isEngineEvent(item)) && decimal(value.lagged) && Object.keys(value).length === 3;
		case 'done': return true;
		default: return false;
	}
}
