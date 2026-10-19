export interface Grants { view: boolean; control: boolean; clipboard: boolean; transfer: boolean; }

export type OperatingSystem = 'macos' | 'windows' | 'linux' | 'other';

/** Whether this computer can reach a paired one now; `notAccepting` means reachable but paused by its owner. */
export type DevicePresence = 'connected' | 'connecting' | 'reconnecting' | 'notAccepting' | 'disconnected';

const devicePresences: readonly DevicePresence[] = ['connected', 'connecting', 'reconnecting', 'notAccepting', 'disconnected'];

/** A capability a paired computer reports on, by its stable name. */
export type RemoteCapability = 'desktopView' | 'desktopControl' | 'clipboard' | 'files' | 'terminal' | 'camera' | 'systemAudio' | 'microphone' | 'systemLock' | 'systemRestart' | 'systemShutdown' | 'systemPrivacy' | 'recording';

/** What a paired computer allows: available, or why not, in the coarse terms it shares. */
export type RemoteStatus = 'available' | 'turnedOff' | 'needsPermission' | 'unsupported' | 'unavailable';

export interface RemoteCapabilityStatus { capability: RemoteCapability; status: RemoteStatus; }

const remoteCapabilities: readonly RemoteCapability[] = ['desktopView', 'desktopControl', 'clipboard', 'files', 'terminal', 'camera', 'systemAudio', 'microphone', 'systemLock', 'systemRestart', 'systemShutdown', 'systemPrivacy', 'recording'];

const remoteStatuses: readonly RemoteStatus[] = ['available', 'turnedOff', 'needsPermission', 'unsupported', 'unavailable'];

function remoteReport(value: unknown): boolean {
	if (!Array.isArray(value) || value.length > remoteCapabilities.length) {
		return false;
	}
	const seen = new Set<unknown>();
	return value.every((entry: unknown) => record(entry) && Object.keys(entry).length === 2
		&& remoteCapabilities.some(name => name === entry.capability) && remoteStatuses.some(name => name === entry.status)
		&& !seen.has(entry.capability) && seen.add(entry.capability).size > 0);
}

export interface SharingPolicy {
	view: boolean;
	control: boolean;
	clipboard: boolean;
	files: boolean;
	terminal: boolean;
	camera: boolean;
	system_audio: boolean;
	microphone: boolean;
	recording: boolean;
	system_actions: boolean;
}

export interface Device {
	os: OperatingSystem | null;
	presence: DevicePresence;
	endpoint_id: string;
	hostname: string | null;
	pet_name: string | null;
	relays: string[];
	direct: string[];
	grants: Grants;
}

/** How a paired device is named for people: its pet name, else its hostname, else its endpoint. */
export function deviceName(device: Pick<Device, 'pet_name' | 'hostname' | 'endpoint_id'>): string {
	return device.pet_name ?? device.hostname ?? device.endpoint_id;
}

export interface Capabilities {
	desktop_capture: boolean; keyboard: boolean; pointer: boolean; clipboard: boolean; file_transfer: boolean;
	audio: boolean; monitors: number; h264_encode: boolean; h264_decode: boolean;
}

export interface SessionInfo { id: string; generation: string; role: 'Controller' | 'Host'; phase: string | { Failed: { message: string } }; peer_id: string; quality?: QualityLimits | null; monitor_width?: number; monitor_height?: number; }

export interface PointerSettings { sensitivity_percent: number; reverse_wheel: boolean; swap_buttons: boolean; }

export interface DisplayInfo {
	id: string;
	origin_x: number;
	origin_y: number;
	width: number;
	height: number;
	rotation_deg: number;
	scale_percent: number;
	revision: string;
}

export interface QualityLimits { max_bitrate_kbps: number; max_frame_rate: number; max_width: number; max_height: number; }

export interface VideoCodecCaps {
	additional?: { codec: 'H265' | 'Vp8' | 'Vp9' | 'Av1'; encode: boolean; decode: boolean; hardware: boolean }[];
	encode_baseline: boolean; encode_high: boolean; decode_baseline: boolean; decode_high: boolean;
	max_width: number; max_height: number; hardware: boolean; device: string; chroma444: boolean;
}

export interface RelayInput { url: string; authenticated: boolean; replacementToken: string; }

export type ServiceKind = 'Desktop' | 'Files' | 'Terminal' | 'Camera' | 'SystemAudio' | 'Microphone' | 'System' | 'ScreenSnapshot';

export interface ServiceSummary {
	id: string; connectionId: string; generation: string; peer_id: string;
	kind: ServiceKind;
	phase: string | { Closed: { cause: string } } | { Failed: { message: string } };
}

export interface SystemPolicy { allow_lock: boolean; allow_restart: boolean; allow_shutdown: boolean; allow_privacy: boolean; }

export interface SystemSupport { lock: boolean; restart: boolean; shutdown: boolean; privacy: boolean; }

export interface TerminalPolicy {
	allow_persistence: boolean; max_sessions: number; max_sessions_per_peer: number;
	history_bytes: number; replay_bytes: number; detached_timeout_secs: number;
}

/** A shell this computer can offer to paired computers, by the id its user picks it with. */
export interface TerminalShell { id: string; label: string }

/** The shells this computer can offer, what automatic resolves to, and the one offered now. */
export interface TerminalShells { shells: TerminalShell[]; automatic: string; current: string | null }

export interface TerminalSession {
	id: string; process_id: number; shell: string; rows: number; columns: number;
	persistent: boolean; attached: boolean;
}

export interface FileJob {
	id: string; peer: string; label: string; direction: 'Upload' | 'Download'; sender: boolean;
	state: 'Preparing' | 'Serving' | 'Transferring' | 'WaitingDecision' | 'Interrupted' | 'Cancelled' | 'Failed' | 'Complete';
	total_bytes: string; done_bytes: string; files_total: string; files_done: string;
	resumed_files: string; skipped_files: string; skipped_symlinks: string;
	waiting_file: string | null; error: string | null;
}

export type EngineEvent =
	| { type: 'hostingChanged'; hosting: boolean }
	| { type: 'devicePaired' | 'deviceAuthorized' | 'deviceRevoked' | 'deviceUnpaired'; endpoint_id: string }
	| { type: 'controlRequested'; endpoint_id: string; check: string }
	/** A paired computer asked this one to restart or shut down; it runs after `remainingMs` unless cancelled. */
	| { type: 'systemActionScheduled'; id: string; action: 'restart' | 'shutdown'; endpoint_id: string; remainingMs: number }
	| { type: 'systemActionEnded'; id: string; state: SystemOperationState }
	| { type: 'addressesUpdated'; endpoint_id: string }
	| { type: 'peerRenamed'; endpoint_id: string; hostname: string | null }
	| { type: 'presenceChanged'; endpoint_id: string; presence: DevicePresence }
	| { type: 'peerAvailabilityChanged'; endpoint_id: string }
	| { type: 'sessionChanged'; id: string; generation: string; role: 'Controller' | 'Host'; phase: SessionPhase; peer_id: string }
	| { type: 'serviceChanged' | 'hostedServiceChanged' | 'desktopClipboardReceived'; service: ServiceSummary }
	| { type: 'qualityUpdated'; id: string; quality: QualityLimits }
	| { type: 'inputFailed'; id: string; message: string }
	| { type: 'keyframeRequested' | 'clipboardReceived' | 'clipboardFilesReceived'; id: string }
	| { type: 'topologyChanged'; revision: string }
	| { type: 'hostLocked'; locked: boolean };

export type SessionPhase = string | { Failed: { message: string } };

export interface SessionPath { relay: boolean; selected: boolean; rtt_ms: string; tx_bytes: string; rx_bytes: string; }

export type VideoCodec = 'h264' | 'h265' | 'vp8' | 'vp9' | 'av1';

export interface VideoFrame { codec: VideoCodec; keyframe: boolean; timestamp_us: string; data: string; display_id: string; revision: string; }

export interface RelayInfo { url: string; authenticated: boolean; }

export interface ControlRequestInfo { endpoint_id: string; check: string; }

export interface EngineSnapshot {
	endpoint_id: string;
	hosting: boolean;
	public_relay: boolean;
	relay_urls: string[];
	devices: Device[];
	capabilities: Capabilities;
	incoming: SessionInfo | null;
	outgoing: SessionInfo | null;
}

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
	| { type: 'sharingPolicy' }
	| { type: 'setSharingPolicy'; policy: Partial<SharingPolicy> }
	| { type: 'systemPolicy' }
	| { type: 'setSystemPolicy'; policy: SystemPolicy }
	| { type: 'systemAction'; sessionId: string; generation: string; action: SystemActionKind }
	| { type: 'systemOperation' | 'cancelSystemAction'; sessionId: string; generation: string; id: string }
	| { type: 'scheduledSystemActions' }
	| { type: 'cancelHostedSystemAction' | 'runHostedSystemActionNow'; id: string }
	| { type: 'terminalPolicy' }
	| { type: 'setTerminalPolicy'; policy: TerminalPolicy }
	| { type: 'terminalShells' | 'hostedTerminalSessions' }
	/** `null` offers the automatic shell. */
	| { type: 'setTerminalShell'; shell: string | null }
	| { type: 'remoteTerminalSessions'; connectionId: string; generation: string }
	| { type: 'closeRemoteTerminalSession'; connectionId: string; generation: string; id: string }
	| { type: 'closeTerminalSession'; id: string }
	| { type: 'subscribeFilesJobs'; sessionId: string; generation: string }
	| { type: 'pollFilesJobs' | 'releaseFilesJobs'; subscriptionId: string }
	| { type: 'filesJobCancel' | 'filesJobResume'; subscriptionId: string; jobId: string }
	| { type: 'createInvitation'; ttlSeconds: number }
	| { type: 'acceptInvitation'; token: string }
	| { type: 'exportPairingFile'; ttlSeconds: number; allowControl: boolean }
	| { type: 'openPairingFile'; bytes: number[] }
	| { type: 'openPairingFileBase64'; data: string }
	| { type: 'confirmPairing'; previewId: string; digits: string }
	| { type: 'revoke' | 'unpair'; endpointId: string }
	| { type: 'setPeerName'; endpointId: string; name: string }
	| { type: 'connect' | 'beginConnect'; endpointId: string }
	| { type: 'disconnect' }
	| { type: 'disconnectSession'; sessionId: string }
	| { type: 'stopHostedPeer'; sessionId: string }
	| { type: 'submitKey'; sessionId: string; generation: string; usage: number; down: boolean }
	| { type: 'submitPointer'; sessionId: string; generation: string; x: number; y: number; display: string; revision: string }
	| { type: 'submitButton'; sessionId: string; generation: string; button: 'Left' | 'Right' | 'Middle' | 'Back' | 'Forward'; down: boolean }
	| { type: 'submitWheel' | 'submitMotion'; sessionId: string; generation: string; dx: number; dy: number }
	| { type: 'submitPreciseWheel'; sessionId: string; generation: string; dx: number; dy: number }
	| { type: 'submitText' | 'submitClipboard'; sessionId: string; generation: string; text: string }
	| { type: 'submitLocalKey'; sessionId: string; generation: string; usage: number; down: boolean; text: string }
	| { type: 'releaseSessionInput'; sessionId: string; generation: string }
	| { type: 'setInputDefaults'; mode: 'Character' | 'Map' | 'Translate'; swap: boolean; pointer: PointerSettings }
	| { type: 'enableFilesystemAccess' }
	| { type: 'startNativeScreenSharing'; displayId: number; width: number; height: number; pixelWidth: number; pixelHeight: number }
	| { type: 'stopNativeScreenSharing' | 'nativeSharingStatus' }
	| { type: 'startRecording'; sessionId: string; generation: string; destination: string; maxBytes: string; maxDurationMs: string; retention?: RecordingRetention; source?: 'desktop' | 'camera' }
	| { type: 'recordingStatus' | 'stopRecording' | 'releaseRecording'; id: string }
	| { type: 'remoteCameraDevices'; sessionId: string; generation: string }
	| { type: 'remoteAudioDevices'; sessionId: string; generation: string; kind: 'Microphone' | 'System' }
	| { type: 'nativeRuntime' | 'enableNativeInput' | 'nativeCameraDevices' | 'nativeAudioDevices' | 'nativeAudioOutputs' | 'powerPolicy' | 'powerStatus' }
	| { type: 'setNativeCameraSources'; ids: string[] }
	| { type: 'setNativeAudioSources'; microphones: string[]; system: string[] }
	| { type: 'setPowerPolicy'; policy: PowerPolicy }
	| { type: 'reportNativePermissions'; screen: boolean; input: boolean; camera: boolean; microphone: boolean }
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
	| { type: 'savedRelays' | 'relayRestartRequired' }
	| { type: 'controlCheck' | 'controlRequest' | 'rejectControlRequest' }
	| { type: 'confirmControlRequest'; digits: string }
	| { type: 'controllerSessions' | 'hostedSessions' | 'hostedServices' }
	| { type: 'peerAvailability'; endpointId: string }
	| { type: 'controllerSession'; sessionId: string }
	| { type: 'forgetControllerSession'; sessionId: string }
	| { type: 'sessionPaths'; sessionId: string }
	| { type: 'topology' }
	| { type: 'sessionTopology'; sessionId: string; generation: string }
	| { type: 'selectDisplays'; sessionId: string; generation: string; ids: string[] }
	| { type: 'captureDisplays' }
	| { type: 'setTopology'; revision: string; displays: DisplayInfo[] }
	| { type: 'setDesktopCodecDefault'; codec: CodecPreference }
	| { type: 'setDesktopQualityDefaults'; quality: QualityLimits }
	| { type: 'updateSessionQuality'; sessionId: string; generation: string; role: 'Controller' | 'Host'; quality: QualityLimits }
	| { type: 'subscribeSessionVideo'; sessionId: string; generation: string }
	| { type: 'pollVideo'; subscriptionId: string }
	| { type: 'releaseVideo'; subscriptionId: string }
	| { type: 'publishVideo'; codec: VideoCodec; sessionId: string; keyframe: boolean; timestampUs: string; data: string };

export type EngineReply =
	| { type: 'restartRequired'; required: boolean }
	| { type: 'snapshot'; snapshot: EngineSnapshot }
	| { type: 'invitation'; token: string }
	| { type: 'pairingFile'; file: { bytes: number[]; check: string; expires_at_ms: string } }
	| { type: 'pairingPreview'; preview: { id: string; endpoint_id: string; check: string; expires_at_ms: string } }
	| { type: 'device'; device: Device }
	| { type: 'session'; session: SessionInfo }
	| { type: 'sessions'; sessions: SessionInfo[] }
	| { type: 'services'; services: ServiceSummary[] }
	| { type: 'peerAvailability'; report: RemoteCapabilityStatus[] | null }
	| { type: 'text'; text: string | null }
	| { type: 'check'; check: string }
	| { type: 'controlRequest'; request: ControlRequestInfo | null }
	| { type: 'nativeSharingStatus'; displays: NativeSharingStatus[] }
	| { type: 'recordingStatus'; status: RecordingStatus }
	| { type: 'nativeDevices'; devices: NativeDevice[] }
	| { type: 'powerPolicy'; policy: PowerPolicy }
	| { type: 'powerStatus'; statuses: PowerStatus[] }
	| { type: 'nativeRuntime'; backends: NativeBackend[]; capabilities: NativeCapability[] }
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
	| { type: 'sharingPolicy'; policy: SharingPolicy }
	| { type: 'systemPolicy'; policy: SystemPolicy; support: SystemSupport }
	| { type: 'systemOperation'; operation: SystemOperation }
	| { type: 'scheduledSystemActions'; actions: ScheduledSystemAction[] }
	| { type: 'terminalPolicy'; policy: TerminalPolicy }
	| { type: 'terminalSessions'; sessions: TerminalSession[] }
	| ({ type: 'terminalShells' } & TerminalShells)
	| { type: 'jobs'; jobs: FileJob[] }
	| { type: 'job'; job: FileJob }
	| { type: 'fileJobsSubscription'; subscriptionId: string }
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
		&& uint(value.width, 65535) && uint(value.height, 65535) && uint(value.rotation_deg, 360) && uint(value.scale_percent, 800)
		&& decimal(value.revision) && Object.keys(value).length === 8;
}

function displays(value: unknown): value is DisplayInfo[] {
	return Array.isArray(value) && value.length <= 16 && value.every((item: unknown) => display(item));
}

export function equalQualityLimits(a: Readonly<QualityLimits>, b: Readonly<QualityLimits>): boolean {
	return a.max_bitrate_kbps === b.max_bitrate_kbps && a.max_frame_rate === b.max_frame_rate && a.max_width === b.max_width && a.max_height === b.max_height;
}

export function isQualityLimits(value: unknown): value is QualityLimits {
	return record(value) && int(value.max_bitrate_kbps, 1, 50000) && int(value.max_frame_rate, 1, 60) && int(value.max_width, 1, 7680)
		&& int(value.max_height, 1, 7680) && Object.keys(value).length === 4;
}

function videoCodecs(value: unknown): value is VideoCodecCaps {
	return record(value) && typeof value.encode_baseline === 'boolean' && typeof value.encode_high === 'boolean'
		&& typeof value.decode_baseline === 'boolean' && typeof value.decode_high === 'boolean'
		&& uint(value.max_width, 65535) && uint(value.max_height, 65535) && typeof value.hardware === 'boolean'
		&& text(value.device) && typeof value.chroma444 === 'boolean' && (value.additional === undefined ? Object.keys(value).length === 9 : Array.isArray(value.additional) && value.additional.length <= 4 && value.additional.every((c: unknown) => record(c) && ['H265', 'Vp8', 'Vp9', 'Av1'].includes(String(c.codec)) && typeof c.encode === 'boolean' && typeof c.decode === 'boolean' && typeof c.hardware === 'boolean' && Object.keys(c).length === 4) && new Set(value.additional.map((c: { codec: string }) => c.codec)).size === value.additional.length && Object.keys(value).length === 10);
}

function relayInput(value: unknown): value is RelayInput {
	return record(value) && text(value.url) && typeof value.authenticated === 'boolean'
		&& typeof value.replacementToken === 'string' && value.replacementToken.length <= 64 * 1024
		&& Object.keys(value).length === 3;
}

function relayInputs(value: unknown): value is RelayInput[] {
	return Array.isArray(value) && value.length <= 16 && value.every((item: unknown) => relayInput(item));
}

function systemPolicy(value: unknown): value is SystemPolicy {
	return record(value) && typeof value.allow_lock === 'boolean' && typeof value.allow_restart === 'boolean'
		&& typeof value.allow_shutdown === 'boolean' && typeof value.allow_privacy === 'boolean' && Object.keys(value).length === 4;
}

export type SystemActionKind = 'lock' | 'restart' | 'shutdown';

export type SystemOperationState = 'admitted' | 'refused' | 'executed' | 'failed' | 'scheduled' | 'cancelled';

/** One lock, restart or shutdown asked of a paired computer, as it stands. */
export interface SystemOperation {
	readonly id: string;
	readonly action: SystemActionKind;
	readonly state: SystemOperationState;
	/** The host's short reason, such as "disabled by host policy". */
	readonly detail: string;
	/** Time left in a restart or shutdown's notice; zero once it ends. */
	readonly noticeRemainingMs: number;
}

/** A restart or shutdown counting down on this computer. */
export interface ScheduledSystemAction {
	readonly id: string;
	readonly action: 'restart' | 'shutdown';
	readonly endpoint_id: string;
	readonly remainingMs: number;
}

const systemActionKinds: readonly string[] = ['lock', 'restart', 'shutdown'];

const systemOperationStates: readonly string[] = ['admitted', 'refused', 'executed', 'failed', 'scheduled', 'cancelled'];

function noticeMs(value: unknown): value is number {
	return typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 4294967295;
}

function systemOperation(value: unknown): value is SystemOperation {
	return record(value) && decimal(value.id) && systemActionKinds.includes(String(value.action))
		&& systemOperationStates.includes(String(value.state)) && typeof value.detail === 'string' && value.detail.length <= 256
		&& noticeMs(value.noticeRemainingMs) && Object.keys(value).length === 5;
}

function scheduledSystemAction(value: unknown): value is ScheduledSystemAction {
	return record(value) && decimal(value.id) && (value.action === 'restart' || value.action === 'shutdown')
		&& text(value.endpoint_id) && noticeMs(value.remainingMs) && Object.keys(value).length === 4;
}

function systemSupport(value: unknown): value is SystemSupport {
	return record(value) && typeof value.lock === 'boolean' && typeof value.restart === 'boolean'
		&& typeof value.shutdown === 'boolean' && typeof value.privacy === 'boolean' && Object.keys(value).length === 4;
}

function serviceKind(value: unknown): value is ServiceSummary['kind'] {
	return value === 'Desktop' || value === 'Files' || value === 'Terminal' || value === 'Camera'
		|| value === 'SystemAudio' || value === 'Microphone' || value === 'System' || value === 'ScreenSnapshot';
}

function sharingPolicyUpdate(value: unknown): boolean {
	return record(value) && Object.keys(value).length > 0 && Object.entries(value).every(([key, field]) => ['view', 'control', 'clipboard', 'files', 'terminal', 'camera', 'system_audio', 'microphone', 'recording', 'system_actions'].includes(key) && typeof field === 'boolean');
}

function sharingPolicy(value: unknown): value is SharingPolicy {
	return record(value) && ['view', 'control', 'clipboard', 'files', 'terminal', 'camera', 'system_audio', 'microphone', 'recording', 'system_actions'].every(key => typeof value[key] === 'boolean') && Object.keys(value).length === 10;
}

function terminalPolicy(value: unknown): value is TerminalPolicy {
	return record(value) && typeof value.allow_persistence === 'boolean' && uint(value.max_sessions, 65535)
		&& uint(value.max_sessions_per_peer, 65535) && uint(value.history_bytes, 4294967295)
		&& uint(value.replay_bytes, 4294967295) && uint(value.detached_timeout_secs, 4294967295)
		&& Object.keys(value).length === 6;
}

/**
 * The category of an engine failure. The addon formats every error as
 * `<Category>: <message>` (`core_error` in crates/vnilink-node), so the
 * category is stable where the message is not.
 */
export function engineErrorCategory(error: unknown): string | undefined {
	return error instanceof Error ? /^([A-Z][A-Za-z]+): /.exec(error.message)?.[1] : undefined;
}

/** A shell id: a path on Unix, a name on Windows. */
export function shellId(value: unknown): value is string {
	return typeof value === 'string' && value.length > 0 && value.length <= 1024 && !value.includes('\0');
}

function terminalShells(value: Record<string, unknown>): boolean {
	return Array.isArray(value.shells) && value.shells.length <= 64
		&& value.shells.every((shell: unknown) => record(shell) && shellId(shell.id) && text(shell.label) && Object.keys(shell).length === 2)
		&& shellId(value.automatic) && (value.current === null || shellId(value.current)) && Object.keys(value).length === 4;
}

function terminalSession(value: unknown): value is TerminalSession {
	return record(value) && text(value.id) && uint(value.process_id, 4294967295) && text(value.shell)
		&& uint(value.rows, 65535) && uint(value.columns, 65535) && typeof value.persistent === 'boolean'
		&& typeof value.attached === 'boolean' && Object.keys(value).length === 7;
}

function terminalSessions(value: unknown): value is TerminalSession[] {
	return Array.isArray(value) && value.length <= 256 && value.every((item: unknown) => terminalSession(item));
}

function fileJobState(value: unknown): value is FileJob['state'] {
	return value === 'Preparing' || value === 'Serving' || value === 'Transferring' || value === 'WaitingDecision'
		|| value === 'Interrupted' || value === 'Cancelled' || value === 'Failed' || value === 'Complete';
}

function fileJob(value: unknown): value is FileJob {
	return record(value) && text(value.id) && text(value.peer) && text(value.label)
		&& (value.direction === 'Upload' || value.direction === 'Download') && typeof value.sender === 'boolean'
		&& fileJobState(value.state) && decimal(value.total_bytes) && decimal(value.done_bytes)
		&& decimal(value.files_total) && decimal(value.files_done) && decimal(value.resumed_files)
		&& decimal(value.skipped_files) && decimal(value.skipped_symlinks)
		&& (value.waiting_file === null || text(value.waiting_file)) && (value.error === null || text(value.error))
		&& Object.keys(value).length === 15;
}

function fileJobs(value: unknown): value is FileJob[] {
	return Array.isArray(value) && value.length <= 256 && value.every((item: unknown) => fileJob(item));
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
		case 'systemActionScheduled': return decimal(value.id) && (value.action === 'restart' || value.action === 'shutdown') && text(value.endpoint_id) && noticeMs(value.remainingMs) && Object.keys(value).length === 5;
		case 'systemActionEnded': return decimal(value.id) && systemOperationStates.includes(String(value.state)) && Object.keys(value).length === 3;
		case 'addressesUpdated': return text(value.endpoint_id) && Object.keys(value).length === 2;
		case 'peerRenamed': return text(value.endpoint_id) && (value.hostname === null || text(value.hostname)) && Object.keys(value).length === 3;
		case 'presenceChanged': return text(value.endpoint_id) && devicePresences.some(presence => presence === value.presence) && Object.keys(value).length === 3;
		case 'peerAvailabilityChanged': return text(value.endpoint_id) && Object.keys(value).length === 2;
		case 'sessionChanged':
			return decimal(value.id) && decimal(value.generation) && sessionRole(value.role) && sessionPhase(value.phase)
				&& text(value.peer_id) && Object.keys(value).length === 6;
		case 'serviceChanged': case 'hostedServiceChanged': case 'desktopClipboardReceived':
			return serviceInfo(value.service) && Object.keys(value).length === 2;
		case 'qualityUpdated': return decimal(value.id) && isQualityLimits(value.quality) && Object.keys(value).length === 3;
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
	return record(value) && ['h264', 'h265', 'vp8', 'vp9', 'av1'].includes(String(value.codec)) && typeof value.keyframe === 'boolean' && decimal(value.timestamp_us) && frameData(value.data)
		&& decimal(value.display_id) && decimal(value.revision) && Object.keys(value).length === 6;
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
	return record(value) && (value.os === null || typeof value.os === 'string' && ['macos', 'windows', 'linux', 'other'].includes(value.os)) && devicePresences.some(presence => presence === value.presence) && Object.keys(value).length === 8 && text(value.endpoint_id) && (value.hostname === null || text(value.hostname)) && (value.pet_name === null || text(value.pet_name)) && strings(value.relays) && strings(value.direct) && record(value.grants) && grants(value.grants);
}

function capabilities(value: unknown): value is Capabilities {
	return record(value) && ['desktop_capture', 'keyboard', 'pointer', 'clipboard', 'file_transfer', 'audio', 'h264_encode', 'h264_decode'].every(key => typeof value[key] === 'boolean') && typeof value.monitors === 'number' && Number.isInteger(value.monitors) && value.monitors >= 0 && value.monitors <= 255;
}

function session(value: unknown): value is SessionInfo | null {
	return value === null || (record(value) && decimal(value.id) && decimal(value.generation) && (value.role === 'Controller' || value.role === 'Host') && text(value.peer_id)
		&& (!('quality' in value) || value.quality === null || isQualityLimits(value.quality))
		&& (!('monitor_width' in value) || uint(value.monitor_width, 65535)) && (!('monitor_height' in value) || uint(value.monitor_height, 65535))
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
		case 'revoke': case 'unpair': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'setPeerName': keys = ['endpointId', 'name']; valid = text(value.endpointId) && text(value.name) && value.name.length <= 256; break;
		case 'connect': case 'beginConnect': case 'connectFiles': case 'beginConnectFiles': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'disconnect': keys = []; valid = true; break;
		case 'disconnectSession': case 'stopHostedPeer': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'submitKey': keys = ['sessionId', 'generation', 'usage', 'down']; valid = decimal(value.sessionId) && decimal(value.generation) && uint(value.usage, 65535) && typeof value.down === 'boolean'; break;
		case 'submitPointer': keys = ['sessionId', 'generation', 'x', 'y', 'display', 'revision']; valid = decimal(value.sessionId) && decimal(value.generation) && int(value.x, -2147483648, 2147483647) && int(value.y, -2147483648, 2147483647) && decimal(value.display) && decimal(value.revision); break;
		case 'submitButton': keys = ['sessionId', 'generation', 'button', 'down']; valid = decimal(value.sessionId) && decimal(value.generation) && pointerButton(value.button) && typeof value.down === 'boolean'; break;
		case 'submitWheel': case 'submitMotion': keys = ['sessionId', 'generation', 'dx', 'dy']; valid = decimal(value.sessionId) && decimal(value.generation) && int(value.dx, -32768, 32767) && int(value.dy, -32768, 32767); break;
		case 'submitPreciseWheel': keys = ['sessionId', 'generation', 'dx', 'dy']; valid = decimal(value.sessionId) && decimal(value.generation) && int(value.dx, -2147483648, 2147483647) && int(value.dy, -2147483648, 2147483647); break;
		case 'submitText': case 'submitClipboard': keys = ['sessionId', 'generation', 'text']; valid = decimal(value.sessionId) && decimal(value.generation) && longText(value.text); break;
		case 'submitLocalKey': keys = ['sessionId', 'generation', 'usage', 'down', 'text']; valid = decimal(value.sessionId) && decimal(value.generation) && uint(value.usage, 65535) && typeof value.down === 'boolean' && text(value.text); break;
		case 'releaseSessionInput': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'setInputDefaults': keys = ['mode', 'swap', 'pointer']; valid = keyboardMode(value.mode) && typeof value.swap === 'boolean' && pointerSettings(value.pointer); break;
		case 'enableFilesystemAccess': keys = []; break;
		case 'startNativeScreenSharing': keys = ['displayId', 'width', 'height', 'pixelWidth', 'pixelHeight']; valid = isNativeScreenSource({ displayId: value.displayId, width: value.width, height: value.height, pixelWidth: value.pixelWidth, pixelHeight: value.pixelHeight }); break;
		case 'nativeSharingStatus': case 'stopNativeScreenSharing': keys = []; valid = true; break;
		case 'remoteCameraDevices': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'remoteAudioDevices': keys = ['sessionId', 'generation', 'kind']; valid = decimal(value.sessionId) && decimal(value.generation) && (value.kind === 'Microphone' || value.kind === 'System'); break;
		case 'recordingStatus': case 'stopRecording': case 'releaseRecording': keys = ['id']; valid = decimal(value.id); break;
		case 'startRecording': keys = ['sessionId', 'generation', 'destination', 'maxBytes', 'maxDurationMs', ...(value.retention === undefined ? [] : ['retention']), ...(value.source === undefined ? [] : ['source'])]; valid = decimal(value.sessionId) && decimal(value.generation) && typeof value.destination === 'string' && value.destination.length > 0 && value.destination.length <= 4096 && !value.destination.includes('\0') && decimal(value.maxBytes) && decimal(value.maxDurationMs) && (value.retention === undefined || recordingRetention(value.retention)) && (value.source === undefined || value.source === 'desktop' || value.source === 'camera'); break;
		case 'setNativeCameraSources': keys = ['ids']; valid = nativeSourceIds(value.ids); break;
		case 'setNativeAudioSources': keys = ['microphones', 'system']; valid = nativeSourceIds(value.microphones) && nativeSourceIds(value.system) && value.microphones.length + value.system.length <= 32 && new Set([...value.microphones, ...value.system]).size === value.microphones.length + value.system.length; break;
		case 'setPowerPolicy': keys = ['policy']; valid = powerPolicy(value.policy); break;
		case 'nativeCameraDevices': case 'nativeAudioDevices': case 'nativeAudioOutputs': case 'powerPolicy': case 'powerStatus': case 'nativeRuntime': case 'enableNativeInput': keys = []; valid = true; break;
		case 'reportNativePermissions': keys = ['screen', 'input', 'camera', 'microphone']; valid = [value.screen, value.input, value.camera, value.microphone].every(flag => typeof flag === 'boolean'); break;
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
		case 'relayRestartRequired': case 'savedRelays': keys = []; valid = true; break;
		case 'controlCheck': case 'controlRequest': case 'rejectControlRequest': keys = []; valid = true; break;
		case 'confirmControlRequest': keys = ['digits']; valid = typeof value.digits === 'string' && /^[0-9]{3}$/.test(value.digits); break;
		case 'controllerSessions': case 'hostedSessions': case 'hostedServices': keys = []; valid = true; break;
		case 'peerAvailability': keys = ['endpointId']; valid = text(value.endpointId); break;
		case 'controllerSession': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'forgetControllerSession': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'sessionPaths': case 'controllerSessionPaths': keys = ['sessionId']; valid = decimal(value.sessionId); break;
		case 'pollEvents': keys = ['subscriptionId', 'maxEvents']; valid = decimal(value.subscriptionId) && typeof value.maxEvents === 'number' && Number.isInteger(value.maxEvents) && value.maxEvents >= 1 && value.maxEvents <= 256; break;
		case 'releaseEvents': keys = ['subscriptionId']; valid = decimal(value.subscriptionId); break;
		case 'sharingPolicy': keys = []; valid = true; break;
		case 'setSharingPolicy': keys = ['policy']; valid = sharingPolicyUpdate(value.policy); break;
		case 'systemPolicy': keys = []; valid = true; break;
		case 'setSystemPolicy': keys = ['policy']; valid = systemPolicy(value.policy); break;
		case 'systemAction': keys = ['sessionId', 'generation', 'action']; valid = decimal(value.sessionId) && decimal(value.generation) && systemActionKinds.includes(String(value.action)); break;
		case 'systemOperation': case 'cancelSystemAction': keys = ['sessionId', 'generation', 'id']; valid = decimal(value.sessionId) && decimal(value.generation) && decimal(value.id); break;
		case 'scheduledSystemActions': keys = []; valid = true; break;
		case 'cancelHostedSystemAction': case 'runHostedSystemActionNow': keys = ['id']; valid = decimal(value.id); break;
		case 'terminalPolicy': keys = []; valid = true; break;
		case 'setTerminalPolicy': keys = ['policy']; valid = terminalPolicy(value.policy); break;
		case 'terminalShells': case 'hostedTerminalSessions': keys = []; valid = true; break;
		case 'setTerminalShell': keys = ['shell']; valid = value.shell === null || shellId(value.shell); break;
		case 'remoteTerminalSessions': keys = ['connectionId', 'generation']; valid = decimal(value.connectionId) && decimal(value.generation); break;
		case 'closeRemoteTerminalSession': keys = ['connectionId', 'generation', 'id']; valid = decimal(value.connectionId) && decimal(value.generation) && text(value.id); break;
		case 'closeTerminalSession': keys = ['id']; valid = text(value.id); break;
		case 'subscribeFilesJobs': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'pollFilesJobs': case 'releaseFilesJobs': keys = ['subscriptionId']; valid = decimal(value.subscriptionId); break;
		case 'filesJobCancel': case 'filesJobResume': keys = ['subscriptionId', 'jobId']; valid = decimal(value.subscriptionId) && decimal(value.jobId); break;
		case 'topology': keys = []; valid = true; break;
		case 'sessionTopology': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'selectDisplays': keys = ['sessionId', 'generation', 'ids']; valid = decimal(value.sessionId) && decimal(value.generation) && decimals(value.ids); break;
		case 'captureDisplays': keys = []; valid = true; break;
		case 'setTopology': keys = ['revision', 'displays']; valid = decimal(value.revision) && displays(value.displays); break;
		case 'setDesktopCodecDefault': keys = ['codec']; valid = isCodecPreference(value.codec); break;
		case 'setDesktopQualityDefaults': keys = ['quality']; valid = isQualityLimits(value.quality); break;
		case 'updateSessionQuality': keys = ['sessionId', 'generation', 'role', 'quality']; valid = decimal(value.sessionId) && decimal(value.generation) && sessionRole(value.role) && isQualityLimits(value.quality); break;
		case 'subscribeSessionVideo': keys = ['sessionId', 'generation']; valid = decimal(value.sessionId) && decimal(value.generation); break;
		case 'pollVideo': case 'releaseVideo': keys = ['subscriptionId']; valid = decimal(value.subscriptionId); break;
		case 'publishVideo': keys = ['codec', 'sessionId', 'keyframe', 'timestampUs', 'data']; valid = ['h264', 'h265', 'vp8', 'vp9', 'av1'].includes(String(value.codec)) && decimal(value.sessionId) && typeof value.keyframe === 'boolean' && decimal(value.timestampUs) && frameData(value.data); break;
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
		case 'services': return Array.isArray(value.services) && value.services.every((service: unknown) => serviceInfo(service)) && Object.keys(value).length === 2;
		case 'peerAvailability': return (value.report === null || remoteReport(value.report)) && Object.keys(value).length === 2;
		case 'text': return (value.text === null || longText(value.text)) && Object.keys(value).length === 2;
		case 'check': return text(value.check) && Object.keys(value).length === 2;
		case 'controlRequest': return controlRequest(value.request) && Object.keys(value).length === 2;
		case 'nativeSharingStatus': return Array.isArray(value.displays) && value.displays.length <= 16 && value.displays.every(sharingStatus) && Object.keys(value).length === 2;
		case 'recordingStatus': return recordingStatus(value.status) && Object.keys(value).length === 2;
		case 'nativeDevices': return Array.isArray(value.devices) && value.devices.length <= 256 && value.devices.every(nativeDevice) && Object.keys(value).length === 2;
		case 'powerPolicy': return powerPolicy(value.policy) && Object.keys(value).length === 2;
		case 'powerStatus': return Array.isArray(value.statuses) && value.statuses.length <= 2 && value.statuses.every(powerStatus) && Object.keys(value).length === 2;
		case 'nativeRuntime': return nativeRuntime(value) && Object.keys(value).length === 3;
		case 'keyboardMode': return keyboardMode(value.mode) && Object.keys(value).length === 2;
		case 'swap': return typeof value.enabled === 'boolean' && Object.keys(value).length === 2;
		case 'pointerSettings': return pointerSettings(value.settings) && Object.keys(value).length === 2;
		case 'displays': return displays(value.displays) && Object.keys(value).length === 2;
		case 'displayIds': return decimals(value.displays) && Object.keys(value).length === 2;
		case 'paths': return Array.isArray(value.paths) && value.paths.every((item: unknown) => sessionPath(item)) && Object.keys(value).length === 2;
		case 'restartRequired': return typeof value.required === 'boolean' && Object.keys(value).length === 2;
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
		case 'sharingPolicy': return sharingPolicy(value.policy) && Object.keys(value).length === 2;
		case 'systemPolicy': return systemPolicy(value.policy) && systemSupport(value.support) && Object.keys(value).length === 3;
		case 'systemOperation': return systemOperation(value.operation) && Object.keys(value).length === 2;
		case 'scheduledSystemActions': return Array.isArray(value.actions) && value.actions.length <= 64 && value.actions.every((action: unknown) => scheduledSystemAction(action)) && Object.keys(value).length === 2;
		case 'terminalPolicy': return terminalPolicy(value.policy) && Object.keys(value).length === 2;
		case 'terminalSessions': return terminalSessions(value.sessions) && Object.keys(value).length === 2;
		case 'terminalShells': return terminalShells(value);
		case 'jobs': return fileJobs(value.jobs) && Object.keys(value).length === 2;
		case 'job': return fileJob(value.job) && Object.keys(value).length === 2;
		case 'fileJobsSubscription': return decimal(value.subscriptionId) && Object.keys(value).length === 2;
		case 'done': return true;
		default: return false;
	}
}

export const nativeCapabilityNames = ['desktopView', 'desktopControl', 'clipboard', 'files', 'terminal', 'camera', 'systemAudio', 'microphone', 'lock', 'restart', 'shutdown', 'privacy', 'recording'] as const;

export type NativeCapabilityName = typeof nativeCapabilityNames[number];

export interface NativeBackend { readonly name: string; readonly ready: boolean; readonly error: string | null; }

export interface NativeCapability { readonly name: NativeCapabilityName; readonly available: boolean; readonly summary: string; }

function nativeRuntime(value: Record<string, unknown>): boolean {
	return Array.isArray(value.backends) && value.backends.length <= 32 && value.backends.every((backend: unknown) => record(backend) && typeof backend.name === 'string' && backend.name.length <= 64 && typeof backend.ready === 'boolean' && (backend.error === null || typeof backend.error === 'string' && backend.error.length <= 512) && Object.keys(backend).length === 3)
		&& Array.isArray(value.capabilities) && value.capabilities.length === nativeCapabilityNames.length && value.capabilities.every((capability: unknown) => record(capability) && typeof capability.name === 'string' && nativeCapabilityNames.some(name => name === capability.name) && typeof capability.available === 'boolean' && typeof capability.summary === 'string' && capability.summary.length <= 512 && Object.keys(capability).length === 3)
		&& new Set(value.capabilities.map((capability: unknown) => record(capability) ? capability.name : undefined)).size === nativeCapabilityNames.length;
}

export interface NativeDevice { readonly id: string; readonly name: string; readonly kind: 'camera' | 'microphone' | 'system' | 'output'; }

export interface PowerPolicy {
	incoming_display: boolean;
	incoming_system: boolean;
	outgoing_display: boolean;
	outgoing_system: boolean;
	incoming_idle_secs: number;
	outgoing_idle_secs: number;
	lock_after_control: boolean;
}

export interface PowerStatus { readonly kind: 'Display' | 'System'; readonly owners: number; readonly phase: 'Idle' | 'Unavailable' | 'Starting' | 'Active' | 'Failed'; readonly error: string | null; }

function nativeSourceIds(value: unknown): value is string[] {
	return Array.isArray(value) && value.length <= 32 && value.every((id: unknown) => typeof id === 'string' && id.length > 0 && id.length <= 2048 && !id.includes('\0')) && new Set(value).size === value.length;
}

function nativeDevice(value: unknown): value is NativeDevice {
	return record(value) && typeof value.id === 'string' && value.id.length > 0 && value.id.length <= 2048 && typeof value.name === 'string' && value.name.length <= 2048 && ['camera', 'microphone', 'system', 'output'].some(kind => kind === value.kind) && Object.keys(value).length === 3;
}

function powerPolicy(value: unknown): value is PowerPolicy {
	return record(value) && ['incoming_display', 'incoming_system', 'outgoing_display', 'outgoing_system', 'lock_after_control'].every(key => typeof value[key] === 'boolean') && uint(value.incoming_idle_secs, 86400) && uint(value.outgoing_idle_secs, 86400) && Object.keys(value).length === 7;
}

function powerStatus(value: unknown): value is PowerStatus {
	return record(value) && (value.kind === 'Display' || value.kind === 'System') && uint(value.owners, 4294967295) && ['Idle', 'Unavailable', 'Starting', 'Active', 'Failed'].some(phase => phase === value.phase) && (value.error === null || typeof value.error === 'string' && value.error.length <= 512) && Object.keys(value).length === 4;
}

export interface RecordingRetention { readonly maxSegments: number; readonly maxTotalBytes: string; }

export interface RecordingStatus { readonly id: string; readonly phase: 'Starting' | 'Recording' | 'Finished' | 'Failed'; readonly failure: 'Startup' | 'Media' | 'Storage' | 'Limit' | 'SlowStorage' | 'FormatChanged' | null; readonly frames: string; readonly bytes: string; readonly durationMs: string; readonly segments: number; }

function recordingRetention(value: unknown): value is RecordingRetention {
	return record(value) && uint(value.maxSegments, 4294967295) && decimal(value.maxTotalBytes) && Object.keys(value).length === 2;
}

function recordingStatus(value: unknown): value is RecordingStatus {
	return record(value) && decimal(value.id) && ['Starting', 'Recording', 'Finished', 'Failed'].some(phase => phase === value.phase) && (value.failure === null || ['Startup', 'Media', 'Storage', 'Limit', 'SlowStorage', 'FormatChanged'].some(failure => failure === value.failure)) && decimal(value.frames) && decimal(value.bytes) && decimal(value.durationMs) && uint(value.segments, 4294967295) && Object.keys(value).length === 7;
}

export interface NativeSharingStatus { readonly displayId: number; readonly phase: 'Starting' | 'Sharing' | 'Failed'; readonly error: string | null; }

function sharingStatus(value: unknown): value is NativeSharingStatus {
	return record(value) && uint(value.displayId, 4294967295) && ['Starting', 'Sharing', 'Failed'].some(phase => phase === value.phase) && (value.error === null || typeof value.error === 'string' && value.error.length <= 512) && Object.keys(value).length === 3;
}

export interface NativeScreenSource {
	readonly displayId: number;
	readonly width: number;
	readonly height: number;
	readonly pixelWidth: number;
	readonly pixelHeight: number;
}

export function isNativeScreenSource(value: unknown): value is NativeScreenSource {
	return record(value) && typeof value.displayId === 'number' && uint(value.displayId, 4294967295) && value.displayId > 0 && [value.width, value.height, value.pixelWidth, value.pixelHeight].every(size => typeof size === 'number' && uint(size, 65535) && size >= 2) && Object.keys(value).length === 5;
}

export function isNativeScreenSources(value: unknown): value is NativeScreenSource[] {
	return Array.isArray(value) && value.length <= 16 && value.every(isNativeScreenSource);
}

export type CodecPreference = 'auto' | VideoCodec;

export function isCodecPreference(value: unknown): value is CodecPreference {
	return typeof value === 'string' && ['auto', 'h264', 'h265', 'vp8', 'vp9', 'av1'].includes(value);
}
