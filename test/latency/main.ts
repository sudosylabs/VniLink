/**
 * One side of the glass-to-glass latency measurement (`npm run measure:latency`).
 *
 * `VNILINK_LATENCY_ROLE=host` hosts this computer's screen from a temporary
 * engine and shows the stimulus strip across the top of the main display.
 * `VNILINK_LATENCY_ROLE=viewer` views that screen in a real workbench window
 * below the strip. The two pair through files in `VNILINK_LATENCY_DIR`, which
 * the runner in `build/lib/latency.ts` owns, and quit when it writes `done`.
 */
import { execFile } from 'node:child_process';
import { app, BrowserWindow, screen } from 'electron';
import { appendFileSync, existsSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { isEngineReply, record } from '../../src/platform/engine/common/engine.js';
import { TestApplication } from '../automation/application.js';
import type { WindowDriver } from '../automation/driver.js';
import { CELLS, stimulusPage } from './stimulus.js';

const role = process.env.VNILINK_LATENCY_ROLE;

const directory = process.env.VNILINK_LATENCY_DIR;

const output = process.env.VNILINK_LATENCY_OUTPUT;

if ((role !== 'host' && role !== 'viewer') || !directory || !output) {
	throw new Error('VNILINK_LATENCY_ROLE (host or viewer), VNILINK_LATENCY_DIR and VNILINK_LATENCY_OUTPUT are required');
}

const rendezvous = directory;

const evidence = output;

/** Publishes `value` under `name` for the other side, whole or not at all. */
function publish(name: string, value: unknown): void {
	const path = join(rendezvous, `${name}.json`);
	writeFileSync(`${path}.partial`, JSON.stringify(value));
	renameSync(`${path}.partial`, path);
}

/** What the other side published under `name`, waiting up to `timeoutMs`. */
async function expect(name: string, timeoutMs: number): Promise<Record<string, unknown>> {
	const path = join(rendezvous, `${name}.json`);
	const deadline = Date.now() + timeoutMs;
	while (!existsSync(path)) {
		if (Date.now() > deadline) {
			throw new Error(`${name} did not arrive within ${String(timeoutMs / 1000)} s`);
		}
		await new Promise(resolve => setTimeout(resolve, 100));
	}
	const value: unknown = JSON.parse(readFileSync(path, 'utf8'));
	if (!record(value)) {
		throw new Error(`${name} is not an object`);
	}
	return value;
}

/** Seconds of CPU this application's processes have used, by kind: the engine and other utility processes by name. */
function cpuSeconds(): Record<string, number> {
	const seconds: Record<string, number> = {};
	for (const metric of app.getAppMetrics()) {
		const kind = metric.type === 'Utility' && metric.name ? metric.name : metric.type;
		seconds[kind] = (seconds[kind] ?? 0) + (metric.cpu.cumulativeCPUUsage ?? 0);
	}
	return seconds;
}

/** CPU used between two `cpuSeconds` samples, in cores, by kind. */
function cores(start: Record<string, number>, end: Record<string, number>, wallSeconds: number): Record<string, number> {
	return Object.fromEntries(Object.entries(end).map(([kind, used]) => [kind, (used - (start[kind] ?? 0)) / wallSeconds]));
}

async function engine(driver: WindowDriver, command: Record<string, unknown>): Promise<Record<string, unknown>> {
	const reply: unknown = await driver.window.webContents.executeJavaScript(`window.driver.engine.execute(${JSON.stringify(command)})`);
	if (!isEngineReply(reply) || !record(reply)) {
		throw new Error(`The engine did not answer ${String(command.type)}`);
	}
	return reply;
}

async function invitation(driver: WindowDriver): Promise<string> {
	const reply = await engine(driver, { type: 'createInvitation', ttlSeconds: 300 });
	if (reply.type !== 'invitation' || typeof reply.token !== 'string') {
		throw new Error('No invitation');
	}
	return reply.token;
}

/** The strip's cell width in CSS pixels and the strip's window height. */
function stripGeometry(): { cell: number; height: number; area: Electron.Rectangle } {
	const area = screen.getPrimaryDisplay().workArea;
	const cell = Math.floor(area.width / (CELLS + 4));
	return { cell, height: Math.ceil(cell * 2), area };
}

/** Whether this application may record the screen; asks macOS once when not. */
async function screenRecording(driver: WindowDriver): Promise<boolean> {
	const granted: unknown = await driver.window.webContents.executeJavaScript(`(async () => {
		const snapshot = await window.driver.permissions.execute({ type: 'status' });
		if (snapshot.permissions.some(permission => permission.kind === 'screen' && permission.status === 'granted')) {
			return true;
		}
		// macOS lists the application once it has asked; a refusal is the answer.
		await window.driver.permissions.execute({ type: 'request', kind: 'screen' }).catch(() => undefined);
		return false;
	})()`);
	return granted === true;
}

async function host(driver: WindowDriver): Promise<void> {
	if (!await screenRecording(driver)) {
		throw new Error('Screen Recording is not granted to the development VniLink (.build/electron/VniLink.app). Allow it in System Settings › Privacy & Security › Screen & System Audio Recording, then run again.');
	}
	const snapshot = await engine(driver, { type: 'snapshot' });
	const endpoint = record(snapshot.snapshot) ? snapshot.snapshot.endpoint_id : undefined;
	if (typeof endpoint !== 'string') {
		throw new Error('No endpoint');
	}
	publish('host', { token: await invitation(driver), endpoint });
	const viewer = await expect('viewer', 60000);
	await engine(driver, { type: 'acceptInvitation', token: viewer.token });
	// Sharing is a setting; main writes it to the engine, as the Settings tab does.
	await driver.window.webContents.executeJavaScript(`window.driver.mainProcess.getChannel('configuration').call('updateValue', { key: 'sharing.screen', value: true })`);
	const { cell, height, area } = stripGeometry();
	const stimulus = new BrowserWindow({
		x: area.x, y: area.y, width: area.width, height,
		frame: false, resizable: false, hasShadow: false, show: false,
		webPreferences: { backgroundThrottling: false },
	});
	try {
		await stimulus.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(stimulusPage(cell))}`);
		stimulus.showInactive();
		publish('host-ready', {});
		// The native host's state while the viewer starts, for a start that fails.
		const trace = join(evidence, 'host-start.log');
		await driver.window.webContents.executeJavaScript(`(() => {
			window.latencyEvents = [];
			window.driver.engine.onDidReceiveEvents(batch => {
				for (const event of batch.events) {
					if (event.type !== 'presenceChanged') {
						window.latencyEvents.push(event);
					}
				}
			});
		})()`);
		for (let sample = 0; sample < 40 && !existsSync(join(rendezvous, 'viewer-streaming.json')) && !existsSync(join(rendezvous, 'read.json')); sample++) {
			const events: unknown = await driver.window.webContents.executeJavaScript('window.latencyEvents.splice(0)');
			appendFileSync(trace, `${String(Date.now())} ${JSON.stringify({ status: await engine(driver, { type: 'nativeSharingStatus' }), events })}\n`);
			await new Promise(resolve => setTimeout(resolve, 500));
		}
		// The reader runs as this process's child, so macOS counts it under
		// this application's Screen Recording permission.
		const read = await expect('read', 5 * 60000);
		const [cpuStart, startedAt] = [cpuSeconds(), Date.now()];
		const outcome = await new Promise<{ code: number; output: string }>(resolve => {
			execFile(String(read.reader), [String(read.seconds), String(read.samples)], (error, stdout, stderr) => {
				resolve({ code: error ? 1 : 0, output: `${stdout}${stderr}` });
			});
		});
		publish('host-cpu', cores(cpuStart, cpuSeconds(), (Date.now() - startedAt) / 1000));
		publish('read-done', outcome);
		await expect('done', 5 * 60000);
	} finally {
		stimulus.destroy();
	}
}

async function viewer(driver: WindowDriver): Promise<void> {
	publish('viewer', { token: await invitation(driver) });
	const host = await expect('host', 60000);
	await engine(driver, { type: 'acceptInvitation', token: host.token });
	await expect('host-ready', 60000);
	// As the Devices page does before it shows a computer's screen.
	await engine(driver, { type: 'beginConnectServices', endpointId: host.endpoint });
	// That only starts the connection; the view shows "not connected" if
	// it opens before the connection is up.
	const connected = Date.now() + 30000;
	for (; ;) {
		const snapshot = await engine(driver, { type: 'snapshot' });
		const devices = record(snapshot.snapshot) && Array.isArray(snapshot.snapshot.devices) ? snapshot.snapshot.devices : [];
		if (devices.some(device => record(device) && device.endpoint_id === host.endpoint && device.presence === 'connected')) {
			break;
		}
		if (Date.now() > connected) {
			throw new Error('The viewer did not connect to the host within 30 s');
		}
		await new Promise(resolve => setTimeout(resolve, 200));
	}
	const { height, area } = stripGeometry();
	driver.window.setBounds({ x: area.x, y: area.y + height, width: area.width, height: area.height - height });
	driver.window.showInactive();
	// The device list learns of the pairing from an engine event; the view
	// opens once it has.
	const deadline = Date.now() + 30000;
	const trace = join(evidence, 'viewer-start.log');
	// What the engine reports while the view starts, for a start that fails.
	await driver.window.webContents.executeJavaScript(`(() => {
		window.latencyEvents = [];
		window.driver.engine.onDidReceiveEvents(batch => {
			for (const event of batch.events) {
				if (event.type !== 'presenceChanged') {
					window.latencyEvents.push(event);
				}
			}
		});
	})()`);
	for (let attempt = 0; ; attempt++) {
		const state: unknown = await driver.window.webContents.executeJavaScript(`(async () => {
			// Opened once: selecting the tab again is not needed and is not watched.
			if (!document.querySelector('canvas.desktop-screen')) {
				await window.driver.commands.executeCommand('vnilink.desktop.view', ${JSON.stringify(host.endpoint)});
			}
			await new Promise(resolve => setTimeout(resolve, 250));
			return {
				role: document.querySelector('.desktop-status-role')?.textContent ?? '',
				// The stats line fills once pictures are drawn and stays empty otherwise.
				stats: document.querySelector('.desktop-status-stats')?.textContent ?? '',
				events: window.latencyEvents.splice(0),
			};
		})()`);
		appendFileSync(trace, `${String(Date.now())} ${JSON.stringify(state)}\n`);
		if (attempt % 10 === 0) {
			await driver.screenshot(`viewer-start-${String(attempt)}`);
		}
		if (record(state) && typeof state.stats === 'string' && state.stats.length > 0) {
			break;
		}
		if (Date.now() > deadline) {
			await driver.screenshot('viewer-no-picture');
			throw new Error('The viewer did not show a picture within 30 s');
		}
	}
	await driver.screenshot('viewer-streaming');
	publish('viewer-streaming', {});
	// The view's own stage times, once a second, as it shows them: the
	// runner sets them against the total read from the screen.
	const stages = join(evidence, 'stages.jsonl');
	// CPU over the reader's run, as the host measures its own.
	let cpuStart: [Record<string, number>, number] | undefined;
	while (!existsSync(join(rendezvous, 'done.json'))) {
		if (!cpuStart && existsSync(join(rendezvous, 'read.json'))) {
			cpuStart = [cpuSeconds(), Date.now()];
		}
		if (cpuStart && existsSync(join(rendezvous, 'read-done.json')) && !existsSync(join(rendezvous, 'viewer-cpu.json'))) {
			publish('viewer-cpu', cores(cpuStart[0], cpuSeconds(), (Date.now() - cpuStart[1]) / 1000));
		}
		const text: unknown = await driver.window.webContents.executeJavaScript(`document.querySelector('.desktop-status-stats')?.title ?? ''`);
		if (typeof text === 'string' && text.length > 0) {
			appendFileSync(stages, `${JSON.stringify({ at: Date.now(), text })}\n`);
		}
		await new Promise(resolve => setTimeout(resolve, 1000));
	}
}

const application = new TestApplication({ showWindow: false, capture: role === 'host', timeoutMs: 30 * 60000 });

void application.run((_window, driver) => role === 'host' ? host(driver) : viewer(driver));
