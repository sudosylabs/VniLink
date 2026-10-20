import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({
	stdin: {
		contents: `
			export { FilesService, filesRefusal, filesOpenFailure } from './src/workbench/services/files/filesServiceImpl.js';
			export { engineErrorCategory } from './src/platform/engine/common/engine.js';
			export { folderName, formatSize, itemCount, jobStatus } from './src/workbench/contrib/files/filesText.js';
		`,
		loader: 'ts',
		resolveDir: process.cwd(),
	},
	bundle: true,
	write: false,
	format: 'esm',
	platform: 'node',
	tsconfig: 'tsconfig.json',
	plugins: [{
		name: 'stubs',
		setup(builder) {
			builder.onResolve({ filter: /(dataChannelClient|extensions)\.js$/ }, args => ({ path: args.path, namespace: 'stub' }));
			builder.onLoad({ filter: /.*/, namespace: 'stub' }, () => ({
				contents: 'export const DataChannelClient = { open() { throw new Error("no channel in tests"); } }; export const registerSingleton = () => {}; export const InstantiationType = {};',
				loader: 'js',
			}));
		},
	}],
});

const { FilesService, filesRefusal, filesOpenFailure, engineErrorCategory, folderName, formatSize, itemCount, jobStatus } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

const job = (id, state) => ({ id, peer: 'p', label: 'a.txt', direction: 'Upload', sender: true, state, total_bytes: '100', done_bytes: '40', files_total: '1', files_done: '0', resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, error: null, destination: null });

/** A paired computer with Files shared, a channel whose answers the test chooses, and a record of what was asked. */
function fixture({ status = 'available', answer } = {}) {
	const asked = [];
	const opened = [];
	const engine = {
		available: true,
		execute: async command => command.type === 'controllerSessions'
			? { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active' }] }
			: { type: 'done' },
	};
	const devices = { availability: async () => [{ capability: 'files', status }] };
	const logged = [];
	const service = new FilesService(engine, devices, { warn: message => logged.push(message) });
	service.openChannel = async (request, consume, onError) => {
		const channel = {
			request, consume, onError, closed: false,
			fileOperation: async operation => {
				asked.push(operation);
				return answer?.(operation) ?? (operation.type === 'jobs' ? { ok: [] } : { ok: null });
			},
			close() {
				channel.closed = true;
			},
		};
		opened.push(channel);
		return channel;
	};
	return { service, asked, opened };
}

test('Files that is turned off there says so, and nothing opens', async () => {
	const { service, opened } = fixture({ status: 'turnedOff' });
	assert.deepEqual(await service.open('peer'), { failure: 'turnedOff' });
	assert.equal(opened.length, 0);
	assert.equal(filesRefusal('needsPermission'), 'needsPermission');
	assert.equal(filesOpenFailure(new Error('TransferDenied: off')), 'turnedOff');
	assert.equal(filesOpenFailure(new Error('StaleSession: gone')), 'disconnected');
});

test('tabs for one computer share its connection, and its transfers report themselves', async () => {
	const { service, opened, asked } = fixture({ answer: operation => operation.type === 'jobs' ? { ok: [job('3', 'Complete')] } : undefined });
	const [first, second] = await Promise.all([service.open('peer'), service.open('peer')]);
	assert.equal(first.session, second.session);
	assert.equal(opened.length, 1);
	assert.deepEqual(asked.map(operation => operation.type), ['adopt', 'jobs'], 'a new connection first carries on with what a lost one left');
	assert.deepEqual(opened[0].request, { kind: 'files', sessionId: '4', generation: '2' });
	const session = first.session;
	assert.deepEqual(session.jobs.map(item => item.id), ['3']);
	let changes = 0;
	session.onDidChangeJobs(() => changes++);
	await opened[0].consume({ type: 'data', sequence: 1, kind: 'jobs', metadata: JSON.stringify({ jobs: [job('5', 'Transferring')] }) });
	assert.equal(changes, 1);
	assert.deepEqual(session.jobs.map(item => [item.id, item.state]), [['3', 'Complete'], ['5', 'Transferring']]);
	// A transfer still running keeps the connection after every tab lets go.
	session.release();
	session.release();
	assert(!opened[0].closed);
	await opened[0].consume({ type: 'data', sequence: 2, kind: 'jobs', metadata: JSON.stringify({ jobs: [job('5', 'Complete')] }) });
	assert(opened[0].closed, 'an idle connection stayed open');
	assert(session.ended);
});

test('a refusal from the other computer is an error naming its reason, and the connection lives', async () => {
	const { service, asked } = fixture({ answer: operation => operation.type === 'rename' ? { error: 'FileExists: the name is taken' } : undefined });
	const { session } = await service.open('peer');
	const folder = { root: '0', path: 'Users/abass' };
	const entry = { name: 'a.txt', kind: 'file', size: '1', mtimeMs: '0', token: { dev: '1', ino: '2' } };
	const error = await session.remote.rename(folder, entry, 'b.txt').catch(failure => failure);
	assert.equal(engineErrorCategory(error), 'FileExists');
	assert(!session.ended);
	await session.copy('local', [{ folder, name: 'a.txt' }, { folder, name: 'Photos' }, { folder, name: 'a.txt' }], { root: '2', path: 'Users/hammed/Downloads' });
	assert.deepEqual(asked.filter(operation => operation.type === 'submit').map(operation => [operation.direction, operation.srcPath, operation.names, operation.dstPath]), [
		['upload', 'Users/abass', ['a.txt', 'Photos'], 'Users/hammed/Downloads'],
	], 'a selection is one transfer, each item once');
	await session.copy('remote', [{ folder: { root: '2', path: '' }, name: 'x' }], folder);
	assert.deepEqual(asked.at(-1), { type: 'submit', direction: 'download', srcRoot: '2', srcPath: '', names: ['x'], dstRoot: '0', dstPath: 'Users/abass' });
	// Items dropped from two folders are a transfer each.
	await session.copy('local', [{ folder, name: 'a.txt' }, { folder: { root: '0', path: 'Users/abass/Desktop' }, name: 'b.txt' }, { folder, name: 'c.txt' }], { root: '2', path: '' });
	assert.deepEqual(asked.slice(-2).map(operation => [operation.srcPath, operation.names]), [['Users/abass', ['a.txt', 'c.txt']], ['Users/abass/Desktop', ['b.txt']]]);
	session.release();
});

test('a folder listing reads every page', async () => {
	const pages = {
		'0': { entries: [{ name: 'a', kind: 'dir', size: '0', mtimeMs: '0', token: { dev: null, ino: null } }], total: '2', nextOffset: '1', truncated: false, skipped: '0' },
		'1': { entries: [{ name: 'b', kind: 'file', size: '9', mtimeMs: '0', token: { dev: null, ino: null } }], total: '2', nextOffset: null, truncated: false, skipped: '1' },
	};
	const { service } = fixture({ answer: operation => operation.type === 'list' ? { ok: pages[operation.offset] } : undefined });
	const { session } = await service.open('peer');
	const listing = await session.remote.list({ root: '0', path: '' });
	assert.deepEqual(listing.entries.map(entry => entry.name), ['a', 'b']);
	assert.equal(listing.skipped, 1);
	session.release();
});

test('sizes, folders and transfers read as a file manager says them', () => {
	assert.equal(formatSize(4), '4 bytes');
	assert.equal(formatSize(4000), '4 KB');
	assert.equal(formatSize(38_200_000), '38.2 MB');
	assert.equal(formatSize(412_000_000), '412 MB');
	assert.equal(formatSize(1_100_000_000), '1.1 GB');
	const places = { roots: [{ id: '0', name: '/' }, { id: '2', name: 'C:' }], folders: [] };
	assert.equal(folderName({ root: '0', path: 'Users/hammed/Downloads' }, places, 'This Mac'), 'Downloads');
	assert.equal(folderName({ root: '0', path: '' }, places, 'This Mac'), 'This Mac', 'the whole disk takes its computer\'s name');
	assert.equal(folderName({ root: '2', path: '' }, places, 'Sudosy'), 'C:');
	assert.equal(itemCount(1), '1 item');
	assert.equal(itemCount(3), '3 items');
	const running = job('1', 'Transferring');
	assert.equal(jobStatus(running, undefined), '40 bytes of 100 bytes');
	assert.equal(jobStatus({ ...running, done_bytes: '500000000', total_bytes: '1500000000' }, 20_000_000), '500 MB of 1.5 GB · 20 MB/s · 50 s left');
	assert.equal(jobStatus({ ...running, state: 'WaitingDecision', waiting_file: 'Reports/q3.xlsx' }), '“q3.xlsx” is already there');
	assert.equal(jobStatus({ ...running, state: 'Interrupted' }), 'Stopped at 40 bytes of 100 bytes');
	assert.equal(jobStatus({ ...running, state: 'Interrupted', done_bytes: '0' }), 'Stopped: the connection was lost');
	assert.equal(jobStatus({ ...running, state: 'Failed', error: 'there is not enough space' }), 'there is not enough space');
	assert.equal(jobStatus({ ...running, state: 'Complete', files_total: '86', total_bytes: '212000000' }), '86 files, 212 MB, checked');
});
