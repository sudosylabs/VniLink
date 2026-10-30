// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter, type Event } from '../../../base/common/event.js';
import { Disposable, toDisposable, type IDisposable } from '../../../base/common/lifecycle.js';
import type { DataChannelRequest, DataPacket } from '../../../platform/engine/common/dataChannel.js';
import { engineErrorCategory, type RemoteStatus } from '../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { isFileJob, isFileJobs, isFileJobUpdate, isFilePage, isFilePlaces, type FileAnswer, type FileConflictChoice, type FileEntry, type FileJob, type FileOperation, type FilePlaces } from '../../../platform/engine/common/files.js';
import { DataChannelClient } from '../../../platform/engine/electron-browser/dataChannelClient.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { ILogService } from '../../../platform/log/common/log.js';
import type { FilesPane, FolderLocation } from '../../../platform/state/common/uiState.js';
import { activeControllerSession } from '../devices/controllerSession.js';
import { IDeviceService } from '../devices/deviceService.js';
import { IFilesService, isUnfinished, type FilesFailure, type FilesItem, type FilesListing, type IFilesDisk, type IFilesSession } from './filesService.js';

/** The engine's largest listing; past it a folder reports `truncated`. */
const MAX_LISTED = 20_000;

interface FilesChannel {
	fileOperation(operation: FileOperation): Promise<FileAnswer>;
	close(): void;
}

export type FilesChannelOpener = (
	request: Omit<DataChannelRequest, 'requestId'>,
	consume: (packet: DataPacket) => Promise<void>,
	onError: (error: Error) => void,
) => Promise<FilesChannel>;

/** What the other computer reports about Files, as a reason it can't be opened. */
export function filesRefusal(status: RemoteStatus | undefined): FilesFailure | undefined {
	switch (status) {
		case 'available': return undefined;
		case 'turnedOff': return 'turnedOff';
		case 'needsPermission': return 'needsPermission';
		case 'unavailable': return 'unavailable';
		case 'unsupported':
		case undefined: return 'unsupported';
	}
}

/** Why the channel didn't open, from the engine's error category. */
export function filesOpenFailure(error: unknown): FilesFailure {
	switch (engineErrorCategory(error) ?? '') {
		case 'TransferDenied':
		case 'Unauthorized': return 'turnedOff';
		case 'NoSession':
		case 'StaleSession': return 'disconnected';
		default: return 'failed';
	}
}

/** Every page of a folder, one call per page, until the engine says there are no more. */
async function listAll(page: (offset: string) => Promise<unknown>): Promise<FilesListing> {
	const entries: FileEntry[] = [];
	let offset: string | null = '0';
	let truncated = false;
	let skipped = 0;
	while (offset !== null && entries.length < MAX_LISTED) {
		const reply = await page(offset);
		if (!isFilePage(reply)) {
			throw new Error('Malformed: invalid listing');
		}
		entries.push(...reply.entries);
		truncated = reply.truncated;
		skipped = Number(reply.skipped);
		offset = reply.nextOffset;
	}
	return { entries, truncated, skipped };
}

/** This computer's disk, through its own engine. */
class LocalDisk implements IFilesDisk {
	constructor(private readonly engine: IEngineService) { }

	public watch(folder: FolderLocation, changed: () => void): IDisposable {
		let watch: string | undefined;
		let disposed = false;
		const events = this.engine.onDidReceiveEvents(batch => {
			// Events may have been missed: look again rather than miss a change.
			if (batch.resync || batch.events.some(event => event.type === 'folderChanged' && event.watch === watch)) {
				changed();
			}
		});
		void this.engine.execute({ type: 'watchLocal', root: folder.root, path: folder.path }).then(reply => {
			if (reply.type !== 'watching') {
				return;
			}
			if (disposed) {
				void this.engine.execute({ type: 'unwatchLocal', watch: reply.watch }).catch(() => undefined);
			} else {
				watch = reply.watch;
			}
		}, () => undefined);
		return toDisposable(() => {
			disposed = true;
			events.dispose();
			if (watch !== undefined) {
				void this.engine.execute({ type: 'unwatchLocal', watch }).catch(() => undefined);
			}
		});
	}

	public async places(): Promise<FilePlaces> {
		const reply = await this.engine.execute({ type: 'localPlaces' });
		if (reply.type !== 'places') {
			throw new Error('Malformed: invalid places');
		}
		return reply.places;
	}

	public list(folder: FolderLocation): Promise<FilesListing> {
		return listAll(async offset => {
			const reply = await this.engine.execute({ type: 'listLocal', root: folder.root, path: folder.path, offset });
			return reply.type === 'dirPage' ? reply.page : undefined;
		});
	}

	public async mkdir(folder: FolderLocation, name: string): Promise<void> {
		await this.engine.execute({ type: 'createLocalDir', root: folder.root, path: folder.path, name });
	}

	public async rename(folder: FolderLocation, entry: FileEntry, newName: string): Promise<void> {
		await this.engine.execute({ type: 'renameLocal', root: folder.root, path: folder.path, name: entry.name, token: entry.token, newName });
	}

	public async delete(folder: FolderLocation, entry: FileEntry, trash: boolean): Promise<void> {
		await this.engine.execute({ type: 'deleteLocal', root: folder.root, path: folder.path, name: entry.name, token: entry.token, trash });
	}
}

/** The other computer's disk, through the Files channel. */
class RemoteDisk implements IFilesDisk {
	private current: object | undefined;

	constructor(private readonly ask: (operation: FileOperation) => Promise<unknown>, private readonly onDidChangeFolder: Event<void>) { }

	public watch(folder: FolderLocation, changed: () => void): IDisposable {
		// One folder at a time on the other computer: this one replaces the last.
		const mine = {};
		this.current = mine;
		void this.ask({ type: 'watch', folder: { root: folder.root, path: folder.path } }).catch(() => undefined);
		const listener = this.onDidChangeFolder(() => {
			if (this.current === mine) {
				changed();
			}
		});
		return toDisposable(() => {
			listener.dispose();
			if (this.current === mine) {
				this.current = undefined;
				void this.ask({ type: 'watch', folder: null }).catch(() => undefined);
			}
		});
	}

	public async places(): Promise<FilePlaces> {
		const places = await this.ask({ type: 'places' });
		if (!isFilePlaces(places)) {
			throw new Error('Malformed: invalid places');
		}
		return places;
	}

	public list(folder: FolderLocation): Promise<FilesListing> {
		return listAll(offset => this.ask({ type: 'list', root: folder.root, path: folder.path, offset }));
	}

	public async mkdir(folder: FolderLocation, name: string): Promise<void> {
		await this.ask({ type: 'mkdir', root: folder.root, path: folder.path, name });
	}

	public async rename(folder: FolderLocation, entry: FileEntry, newName: string): Promise<void> {
		await this.ask({ type: 'rename', root: folder.root, path: folder.path, name: entry.name, token: entry.token, newName });
	}

	public async delete(folder: FolderLocation, entry: FileEntry, trash: boolean): Promise<void> {
		await this.ask({ type: 'delete', root: folder.root, path: folder.path, name: entry.name, token: entry.token, trash });
	}
}

class FilesSession extends Disposable implements IFilesSession {
	private readonly jobsChanged = this._register(new Emitter<void>());
	public readonly onDidChangeJobs = this.jobsChanged.event;
	private readonly endedEmitter = this._register(new Emitter<void>());
	public readonly onDidEnd = this.endedEmitter.event;
	private readonly byId = new Map<string, FileJob>();
	private readonly folderChanged = this._register(new Emitter<void>());
	private channel: FilesChannel | undefined;
	private holders = 1;
	private endedValue = false;
	public readonly remote: IFilesDisk;

	constructor(
		public readonly deviceId: string,
		public readonly local: IFilesDisk,
		private readonly onRetire: (session: FilesSession) => void,
		private readonly onLost: (reason: string) => void,
	) {
		super();
		this.remote = new RemoteDisk(operation => this.ask(operation), this.folderChanged.event);
	}

	public get jobs(): readonly FileJob[] {
		return [...this.byId.values()].sort((a, b) => Number(BigInt(a.id) - BigInt(b.id)));
	}

	public get ended(): boolean {
		return this.endedValue;
	}

	/** Attaches the open channel, carries on with what a lost one left interrupted, and reads its jobs. */
	public async attach(channel: FilesChannel): Promise<void> {
		this.channel = channel;
		try {
			await this.ask({ type: 'adopt' });
		} catch {
			// A transfer that cannot be taken over yet stays stopped, with Resume offered where it lands.
		}
		const jobs = await this.ask({ type: 'jobs' });
		if (isFileJobs(jobs)) {
			this.byId.clear();
			for (const job of jobs) {
				this.byId.set(job.id, job);
			}
			this.jobsChanged.fire();
		}
	}

	/** A packet the channel sent unasked: jobs that changed. */
	public consume(packet: DataPacket): void {
		if (packet.kind === 'folderChanged') {
			this.folderChanged.fire();
			return;
		}
		if (packet.kind !== 'jobs') {
			return;
		}
		const update: unknown = JSON.parse(packet.metadata);
		if (!isFileJobUpdate(update)) {
			return;
		}
		for (const job of update.jobs) {
			this.byId.set(job.id, job);
		}
		this.jobsChanged.fire();
		this.retireIfIdle();
	}

	/** Ends the session; `reason` says why for the log, when it wasn't asked to end. */
	public end(reason?: string): void {
		if (this.endedValue) {
			return;
		}
		this.endedValue = true;
		if (reason) {
			this.onLost(reason);
		}
		this.channel?.close();
		this.channel = undefined;
		this.endedEmitter.fire();
		this.onRetire(this);
	}

	public acquire(): void {
		this.holders++;
	}

	public release(): void {
		this.holders = Math.max(0, this.holders - 1);
		this.retireIfIdle();
	}

	public async copy(from: FilesPane, items: readonly FilesItem[], to: FolderLocation): Promise<void> {
		// Items dropped from Finder or Explorer may come from several folders; a pane's selection is of one.
		const folders = new Map<string, { readonly folder: FolderLocation; readonly names: Set<string> }>();
		for (const item of items) {
			const key = `${item.folder.root}:${item.folder.path}`;
			const group = folders.get(key) ?? { folder: item.folder, names: new Set<string>() };
			group.names.add(item.name);
			folders.set(key, group);
		}
		for (const { folder, names } of folders.values()) {
			const job = await this.ask({
				type: 'submit',
				direction: from === 'local' ? 'upload' : 'download',
				srcRoot: folder.root,
				srcPath: folder.path,
				names: [...names],
				dstRoot: to.root,
				dstPath: to.path,
			});
			this.adopt(job);
		}
	}

	public async cancel(jobId: string): Promise<void> {
		this.adopt(await this.ask({ type: 'cancel', jobId }));
	}

	public async resume(jobId: string): Promise<void> {
		this.adopt(await this.ask({ type: 'resume', jobId }));
	}

	public async resolve(jobId: string, choice: FileConflictChoice, applyToRemaining: boolean): Promise<void> {
		this.adopt(await this.ask({ type: 'resolve', jobId, choice, applyToRemaining }));
	}

	public async forget(jobId: string): Promise<void> {
		await this.ask({ type: 'forget', jobId });
		this.byId.delete(jobId);
		this.jobsChanged.fire();
	}

	private adopt(job: unknown): void {
		if (isFileJob(job)) {
			this.byId.set(job.id, job);
			this.jobsChanged.fire();
		}
	}

	/** Asks the other computer; a refusal rejects with its `Category: message`. */
	private async ask(operation: FileOperation): Promise<unknown> {
		if (!this.channel) {
			throw new Error('NoSession: the connection ended');
		}
		const answer = await this.channel.fileOperation(operation);
		if ('error' in answer) {
			throw new Error(answer.error);
		}
		return answer.ok;
	}

	/** No tab holds the session and nothing still needs its connection: end it. */
	private retireIfIdle(): void {
		if (this.holders === 0 && !this.jobs.some(isUnfinished)) {
			this.end();
		}
	}
}

export class FilesService implements IFilesService {
	declare public readonly _serviceBrand: undefined;
	private readonly sessions = new Map<string, FilesSession>();
	private readonly opening = new Map<string, Promise<{ readonly failure: FilesFailure } | { readonly session: IFilesSession }>>();
	protected openChannel: FilesChannelOpener;
	public readonly local: IFilesDisk;

	constructor(
		@IEngineService private readonly engine: IEngineService,
		@IDeviceService private readonly devices: IDeviceService,
		@ILogService private readonly logService: ILogService,
	) {
		this.openChannel = (request, consume, onError) => DataChannelClient.open(engine, request, consume, onError);
		this.local = new LocalDisk(engine);
	}

	public async locateLocal(path: string): Promise<FilesItem | undefined> {
		const reply = await this.engine.execute({ type: 'locateLocal', path });
		if (reply.type !== 'location') {
			throw new Error('Malformed: invalid location');
		}
		return reply.location ? { folder: { root: reply.location.root, path: reply.location.path }, name: reply.location.name } : undefined;
	}

	public async open(deviceId: string): Promise<{ readonly failure: FilesFailure } | { readonly session: IFilesSession }> {
		const existing = this.sessions.get(deviceId);
		if (existing && !existing.ended) {
			existing.acquire();
			return { session: existing };
		}
		// Two tabs opening at once share one connection.
		const pending = this.opening.get(deviceId);
		if (pending) {
			const result = await pending;
			if ('session' in result) {
				(result.session as FilesSession).acquire();
			}
			return result;
		}
		const opening = this.connect(deviceId).finally(() => {
			this.opening.delete(deviceId);
		});
		this.opening.set(deviceId, opening);
		return opening;
	}

	private async connect(deviceId: string): Promise<{ readonly failure: FilesFailure } | { readonly session: IFilesSession }> {
		let report;
		try {
			report = await this.devices.availability(deviceId);
		} catch {
			return { failure: 'failed' };
		}
		if (!report) {
			return { failure: 'disconnected' };
		}
		const refused = filesRefusal(report.find(entry => entry.capability === 'files')?.status);
		if (refused) {
			return { failure: refused };
		}
		const connection = await activeControllerSession(this.engine, deviceId);
		if (!connection) {
			return { failure: 'disconnected' };
		}
		const session = new FilesSession(deviceId, this.local, retired => {
			if (this.sessions.get(deviceId) === retired) {
				this.sessions.delete(deviceId);
			}
			retired.dispose();
		}, reason => {
			this.logService.warn(`Files with ${deviceId} ended: ${reason}`);
		});
		try {
			const channel = await this.openChannel(
				{ kind: 'files', sessionId: connection.id, generation: connection.generation },
				packet => {
					session.consume(packet);
					return Promise.resolve();
				},
				error => {
					session.end(error.message);
				},
			);
			await session.attach(channel);
		} catch (error) {
			session.end();
			return { failure: filesOpenFailure(error) };
		}
		this.sessions.set(deviceId, session);
		return { session };
	}
}

registerSingleton(IFilesService, FilesService, InstantiationType.Delayed);
