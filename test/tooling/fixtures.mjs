/** Test doubles shared by service contract tests. */

const none = () => ({ dispose() { } });

/**
 * An `IEngineService` whose commands go to `api.engineCommand`, so tests
 * can script engine replies as plain functions.
 */
export function fakeEngine(api, available = true) {
	return {
		_serviceBrand: undefined,
		available,
		restartRequired: false,
		onDidChangeAvailability: none,
		onDidChangeRestartRequired: none,
		execute: command => api.engineCommand(command),
		openDataChannel: request => api.openDataChannel(request),
	};
}

/** An `IMainProcessService` serving the given fake channels by name. */
export function fakeMainProcess(channels) {
	return {
		_serviceBrand: undefined,
		getChannel(name) {
			const channel = channels[name];
			if (!channel) {
				throw new Error(`Unexpected channel ${name}`);
			}
			return channel;
		},
		registerChannel() {
			throw new Error('Windows serve no channels in these tests');
		},
	};
}
