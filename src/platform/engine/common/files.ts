/**
 * Files as they cross from the engine: places, listings, entry identities,
 * transfer jobs, and the operations a window asks of the other computer's
 * `files` channel. 64-bit counts and ids are decimal strings.
 */

export type FileFolderKind = 'home' | 'desktop' | 'documents' | 'downloads' | 'pictures';

/** Where a file manager starts on one computer: its roots, then the folders that exist there, home first. */
export interface FilePlaces {
	readonly roots: readonly { readonly id: string; readonly name: string }[];
	readonly folders: readonly { readonly kind: FileFolderKind; readonly root: string; readonly path: string }[];
}

/** An entry's identity; the engine refuses to rename or delete a replaced item. */
export interface FileToken {
	readonly dev: string | null;
	readonly ino: string | null;
}

export interface FileEntry {
	readonly name: string;
	readonly kind: 'file' | 'dir' | 'symlink' | 'other';
	readonly size: string;
	readonly mtimeMs: string;
	readonly token: FileToken;
}

export interface FilePage {
	readonly entries: readonly FileEntry[];
	readonly total: string;
	readonly nextOffset: string | null;
	/** The folder holds more entries than one listing shows. */
	readonly truncated: boolean;
	/** Names that are not valid UTF-8, left out. */
	readonly skipped: string;
}

export interface FileJob {
	id: string; peer: string; label: string; direction: 'Upload' | 'Download'; sender: boolean;
	state: 'Preparing' | 'Serving' | 'Transferring' | 'WaitingDecision' | 'Interrupted' | 'Cancelled' | 'Failed' | 'Complete';
	total_bytes: string; done_bytes: string; files_total: string; files_done: string;
	resumed_files: string; skipped_files: string; skipped_symlinks: string;
	waiting_file: string | null; error: string | null;
	/** Where it lands, when this computer submitted it: the receiving folder and the name it arrives under. */
	destination: FileTarget | null;
}

export type FileConflictChoice = 'skip' | 'replace' | 'keepBoth';

/** Names one entry: its root, its folder's path under the root, and its name. */
export interface FileTarget {
	readonly root: string;
	readonly path: string;
	readonly name: string;
}

/** What a window asks of the other computer's `files` channel. */
export type FileOperation =
	| { readonly type: 'places' }
	| { readonly type: 'list'; readonly root: string; readonly path: string; readonly offset: string }
	| ({ readonly type: 'mkdir' } & FileTarget)
	| ({ readonly type: 'rename'; readonly token: FileToken | null; readonly newName: string } & FileTarget)
	| ({ readonly type: 'delete'; readonly token: FileToken | null } & FileTarget)
	| {
		readonly type: 'submit';
		readonly direction: 'upload' | 'download';
		readonly srcRoot: string;
		readonly srcPath: string;
		readonly dstRoot: string;
		readonly dstPath: string;
	}
	| { readonly type: 'jobs' }
	/** Takes over and resumes the transfers a lost channel to this computer left interrupted. */
	| { readonly type: 'adopt' }
	| { readonly type: 'cancel' | 'resume' | 'forget'; readonly jobId: string }
	| { readonly type: 'resolve'; readonly jobId: string; readonly choice: FileConflictChoice; readonly applyToRemaining: boolean };

/** The channel's answer: the operation's result, or why the engine refused it (`Category: message`). */
export type FileAnswer = { readonly ok: unknown } | { readonly error: string };

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function keys(value: Record<string, unknown>, expected: readonly string[]): boolean {
	const present = Object.keys(value);
	return present.length === expected.length && expected.every(key => present.includes(key));
}

function decimal(value: unknown): value is string {
	return typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
}

/** A name, path or label of at most `max` characters, without NUL. */
function text(value: unknown, max: number): value is string {
	return typeof value === 'string' && value.length <= max && !value.includes('\0');
}

/** A folder path under a root: `/`-separated, no leading or trailing `/`, empty for the root itself. */
export function isFilePath(value: unknown): value is string {
	return text(value, 4096) && (value === '' || (!value.startsWith('/') && !value.endsWith('/') && value.split('/').every(part => part !== '' && part !== '.' && part !== '..')));
}

/** One file or folder name. */
export function isFileName(value: unknown): value is string {
	return text(value, 255) && value !== '' && value !== '.' && value !== '..' && !/[/\\:]/.test(value);
}

function folderKind(value: unknown): value is FileFolderKind {
	return value === 'home' || value === 'desktop' || value === 'documents' || value === 'downloads' || value === 'pictures';
}

export function isFilePlaces(value: unknown): value is FilePlaces {
	return record(value) && keys(value, ['roots', 'folders'])
		&& Array.isArray(value.roots) && value.roots.length <= 32
		&& value.roots.every((root: unknown) => record(root) && keys(root, ['id', 'name']) && decimal(root.id) && text(root.name, 4096))
		&& Array.isArray(value.folders) && value.folders.length <= 16
		&& value.folders.every((folder: unknown) => record(folder) && keys(folder, ['kind', 'root', 'path']) && folderKind(folder.kind) && decimal(folder.root) && isFilePath(folder.path));
}

export function isFileToken(value: unknown): value is FileToken {
	return record(value) && keys(value, ['dev', 'ino']) && (value.dev === null || decimal(value.dev)) && (value.ino === null || decimal(value.ino));
}

function fileEntry(value: unknown): value is FileEntry {
	return record(value) && keys(value, ['name', 'kind', 'size', 'mtimeMs', 'token'])
		&& isFileName(value.name)
		&& (value.kind === 'file' || value.kind === 'dir' || value.kind === 'symlink' || value.kind === 'other')
		&& decimal(value.size) && decimal(value.mtimeMs) && isFileToken(value.token);
}

export function isFilePage(value: unknown): value is FilePage {
	return record(value) && keys(value, ['entries', 'total', 'nextOffset', 'truncated', 'skipped'])
		&& Array.isArray(value.entries) && value.entries.length <= 500 && value.entries.every(fileEntry)
		&& decimal(value.total) && (value.nextOffset === null || decimal(value.nextOffset))
		&& typeof value.truncated === 'boolean' && decimal(value.skipped);
}

function fileJobState(value: unknown): value is FileJob['state'] {
	return value === 'Preparing' || value === 'Serving' || value === 'Transferring' || value === 'WaitingDecision'
		|| value === 'Interrupted' || value === 'Cancelled' || value === 'Failed' || value === 'Complete';
}

export function isFileJob(value: unknown): value is FileJob {
	return record(value) && text(value.id, 32) && decimal(value.id) && text(value.peer, 128) && text(value.label, 8192)
		&& (value.direction === 'Upload' || value.direction === 'Download') && typeof value.sender === 'boolean'
		&& fileJobState(value.state) && decimal(value.total_bytes) && decimal(value.done_bytes)
		&& decimal(value.files_total) && decimal(value.files_done) && decimal(value.resumed_files)
		&& decimal(value.skipped_files) && decimal(value.skipped_symlinks)
		&& (value.waiting_file === null || text(value.waiting_file, 4096)) && (value.error === null || text(value.error, 4096))
		&& (value.destination === null || (record(value.destination) && keys(value.destination, ['root', 'path', 'name']) && target(value.destination)))
		&& Object.keys(value).length === 16;
}

export function isFileJobs(value: unknown): value is FileJob[] {
	return Array.isArray(value) && value.length <= 256 && value.every(isFileJob);
}

function target(value: Record<string, unknown>): boolean {
	return decimal(value.root) && isFilePath(value.path) && isFileName(value.name);
}

export function isFileOperation(value: unknown): value is FileOperation {
	if (!record(value)) {
		return false;
	}
	switch (value.type) {
		case 'places': case 'jobs': case 'adopt':
			return keys(value, ['type']);
		case 'list':
			return keys(value, ['type', 'root', 'path', 'offset']) && decimal(value.root) && isFilePath(value.path) && decimal(value.offset);
		case 'mkdir':
			return keys(value, ['type', 'root', 'path', 'name']) && target(value);
		case 'rename':
			return keys(value, ['type', 'root', 'path', 'name', 'token', 'newName']) && target(value) && (value.token === null || isFileToken(value.token)) && isFileName(value.newName);
		case 'delete':
			return keys(value, ['type', 'root', 'path', 'name', 'token']) && target(value) && (value.token === null || isFileToken(value.token));
		case 'submit':
			return keys(value, ['type', 'direction', 'srcRoot', 'srcPath', 'dstRoot', 'dstPath'])
				&& (value.direction === 'upload' || value.direction === 'download')
				&& decimal(value.srcRoot) && isFilePath(value.srcPath) && value.srcPath !== '' && decimal(value.dstRoot) && isFilePath(value.dstPath);
		case 'cancel': case 'resume': case 'forget':
			return keys(value, ['type', 'jobId']) && decimal(value.jobId);
		case 'resolve':
			return keys(value, ['type', 'jobId', 'choice', 'applyToRemaining']) && decimal(value.jobId)
				&& (value.choice === 'skip' || value.choice === 'replace' || value.choice === 'keepBoth') && typeof value.applyToRemaining === 'boolean';
		default:
			return false;
	}
}

export function isFileAnswer(value: unknown): value is FileAnswer {
	return record(value) && ((keys(value, ['ok'])) || (keys(value, ['error']) && text(value.error, 4096)));
}

/** A pushed update: the jobs of this channel that changed since they were last sent. */
export function isFileJobUpdate(value: unknown): value is { readonly jobs: FileJob[] } {
	return record(value) && keys(value, ['jobs']) && isFileJobs(value.jobs);
}
