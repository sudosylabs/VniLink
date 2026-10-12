import assert from 'node:assert/strict';
import test from 'node:test';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

async function load(entry) {
	const bundle = await build({ entryPoints: [fileURLToPath(new URL(entry, import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'node' });
	return import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
}
const log = await load('../../src/platform/log/common/log.ts');
const { BufferLogger } = await load('../../src/platform/log/common/bufferLog.ts');
const { LogService } = await load('../../src/platform/log/common/logService.ts');
const performance = await load('../../src/base/common/performance.ts');

test('log levels parse, print and order like upstream', () => {
	assert.equal(log.parseLogLevel('trace'), log.LogLevel.Trace);
	assert.equal(log.parseLogLevel('critical'), log.LogLevel.Error);
	assert.equal(log.parseLogLevel('nope'), undefined);
	assert.equal(log.LogLevelToString(log.LogLevel.Warning), 'warn');
	assert.equal(log.DEFAULT_LOG_LEVEL, log.LogLevel.Info);
	assert.ok(log.LogLevel.Trace < log.LogLevel.Debug && log.LogLevel.Debug < log.LogLevel.Error);
});
test('buffered logs flush to the late logger in order', () => {
	const buffer = new BufferLogger(log.LogLevel.Trace);
	buffer.info('first');
	buffer.warn('second');
	const seen = [];
	const target = new log.ConsoleLogger(log.LogLevel.Trace, false);
	const original = console.log;
	console.log = (...args) => void seen.push(args.join(' '));
	try {
		buffer.logger = target;
	} finally {
		console.log = original;
	}
	buffer.dispose();
	assert.ok(seen.some(line => line.includes('first')));
	assert.ok(seen.some(line => line.includes('second')));
});
test('log service multiplexes and follows level changes', () => {
	const primary = new log.ConsoleLogger(log.LogLevel.Error, false);
	const service = new LogService(primary);
	assert.equal(service.getLevel(), log.LogLevel.Error);
	service.setLevel(log.LogLevel.Trace);
	assert.equal(service.getLevel(), log.LogLevel.Trace);
	service.dispose();
});
test('native titlebar height resolves through platform utilities', async () => {
	const windowUtils = await load('../../src/platform/window/common/window.ts');
	assert.equal(windowUtils.getNativeTitlebarHeight.length, 1);
	const height = windowUtils.getNativeTitlebarHeight('25.0.0');
	assert.ok(Number.isInteger(height) && height >= 28 && height <= 35);
	const roots = ['src/app', 'src/platform/windows', 'src/platform/ipc', 'src/workbench'];
	const root = fileURLToPath(new URL('../../', import.meta.url));
	const offenders = [];
	const visit = (dir) => {
		for (const entry of readdirSync(dir)) {
			const full = join(dir, entry);
			if (statSync(full).isDirectory()) {
				visit(full);
			} else if (full.endsWith('.ts')) {
				const content = readFileSync(full, 'utf8');
				if (content.includes('process.platform')) {
					offenders.push(full);
				}
			}
		}
	};
	for (const rel of roots) {
		visit(join(root, rel));
	}
	assert.deepEqual(offenders, []);
});
test('performance marks record and clear', () => {
	performance.mark('vnilink/test-mark');
	const marks = performance.getMarks().filter(mark => mark.name === 'vnilink/test-mark');
	assert.equal(marks.length, 1);
	assert.ok(typeof marks[0].startTime === 'number');
	performance.clearMarks('vnilink/test-mark');
	assert.equal(performance.getMarks().filter(mark => mark.name === 'vnilink/test-mark').length, 0);
});
