import type { BrowserWindowDriver } from '../../src/workbench/services/driver/browser/driver.js';

/** The workbench's smoke-test driver; present only in isolated test launches. */
export function windowDriver(): BrowserWindowDriver {
	const driver: unknown = Reflect.get(window, 'driver');
	if (typeof driver !== 'object' || driver === null) {
		throw new Error('Smoke-test driver unavailable');
	}
	return driver as BrowserWindowDriver;
}
