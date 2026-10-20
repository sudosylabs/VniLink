import type { Event } from '../../../base/common/event.js';
import type { FileConflictChoice, FileEntry, FileJob, FilePlaces } from '../../../platform/engine/common/files.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { FilesPane, FolderLocation } from '../../../platform/state/common/uiState.js';

export const IFilesService = createDecorator<IFilesService>('filesService');

/** Why the other computer's files can't be opened. */
export type FilesFailure = 'turnedOff' | 'needsPermission' | 'unsupported' | 'unavailable' | 'disconnected' | 'failed';

/** One folder's entries, in the engine's order. */
export interface FilesListing {
	readonly entries: readonly FileEntry[];
	/** The folder holds more than one listing shows. */
	readonly truncated: boolean;
	/** Entries left out because their names are not valid UTF-8. */
	readonly skipped: number;
}

/**
 * One computer's disk as a pane uses it. A refusal rejects with an `Error`
 * whose message starts with the engine's category, such as
 * `FileExists: …`; read it with `engineErrorCategory`.
 */
export interface IFilesDisk {
	places(): Promise<FilePlaces>;
	/** Every entry of `folder`, page after page. */
	list(folder: FolderLocation): Promise<FilesListing>;
	mkdir(folder: FolderLocation, name: string): Promise<void>;
	/** Refused when `entry` was replaced since it was listed. */
	rename(folder: FolderLocation, entry: FileEntry, newName: string): Promise<void>;
	/** Deletes `entry` permanently, a folder with everything in it. */
	delete(folder: FolderLocation, entry: FileEntry): Promise<void>;
}

/** An entry to copy: its name in `folder`. */
export interface FilesItem {
	readonly folder: FolderLocation;
	readonly name: string;
}

/**
 * Files with one paired computer: both disks and the transfers between
 * them. Shared by every Files tab for that computer.
 */
export interface IFilesSession {
	readonly deviceId: string;
	readonly local: IFilesDisk;
	readonly remote: IFilesDisk;
	/** This connection's transfers, oldest first. Each names where it lands. */
	readonly jobs: readonly FileJob[];
	readonly onDidChangeJobs: Event<void>;
	/** Whether the connection to the other computer has ended. */
	readonly ended: boolean;
	readonly onDidEnd: Event<void>;
	/** Copies each item from `from`'s disk into `to` on the other one, one transfer per item. */
	copy(from: FilesPane, items: readonly FilesItem[], to: FolderLocation): Promise<void>;
	cancel(jobId: string): Promise<void>;
	resume(jobId: string): Promise<void>;
	resolve(jobId: string, choice: FileConflictChoice, applyToRemaining: boolean): Promise<void>;
	/** Removes a finished transfer from the list. */
	forget(jobId: string): Promise<void>;
	/** Lets go of this holder's share. The connection stays while a transfer is unfinished. */
	release(): void;
}

export interface IFilesService {
	readonly _serviceBrand: undefined;
	/** This computer's disk, which needs no connection. */
	readonly local: IFilesDisk;
	/** Opens Files with `deviceId`, or says why it can't. Each success needs one `release`. */
	open(deviceId: string): Promise<{ readonly failure: FilesFailure } | { readonly session: IFilesSession }>;
}

/** Whether a transfer still needs its connection. Interrupted ones resume on a new one. */
export function isUnfinished(job: FileJob): boolean {
	return job.state === 'Preparing' || job.state === 'Serving' || job.state === 'Transferring' || job.state === 'WaitingDecision';
}
