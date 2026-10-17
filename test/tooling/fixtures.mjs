/** Test doubles shared by service contract tests. */

const none = () => ({ dispose() { } });

/**
 * An `IEngineService` whose commands go to `api.engineCommand`, so tests
 * can script engine replies as plain functions.
 */
export function fakeEngine(api, available = true) {
	const events = fakeEvent();
	const availability = fakeEvent();
	const engine = {
		_serviceBrand: undefined,
		available,
		restartRequired: false,
		onDidChangeAvailability: availability.event,
		onDidChangeRestartRequired: none,
		onDidReceiveEvents: events.event,
		execute: command => api.engineCommand(command),
		openDataChannel: request => api.openDataChannel(request),
		/** Test controls: deliver an event batch, or change availability. */
		fireEvents: batch => events.fire(batch),
		setAvailable: value => {
			engine.available = value;
			availability.fire(value);
		},
		get eventListeners() {
			return events.size;
		},
	};
	return engine;
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

/** A minimal event: listeners added with `event(listener)` receive `fire(value)`. */
export function fakeEvent() {
	const listeners = new Set();
	const event = listener => {
		listeners.add(listener);
		return { dispose: () => listeners.delete(listener) };
	};
	return {
		event, fire: value => listeners.forEach(listener => listener(value)), get size() {
			return listeners.size;
		}
	};
}
