import * as nls from '../../../nls.js';
import type { FileEntry, FileJob, FilePlaces } from '../../../platform/engine/common/files.js';
import type { FolderLocation } from '../../../platform/state/common/uiState.js';

const units = ['bytes', 'KB', 'MB', 'GB', 'TB'] as const;

/** A size as file managers show it: 412 MB, 1.1 GB, 4 KB. */
export function formatSize(bytes: number): string {
	if (bytes < 1000) {
		return nls.localize('files.bytes', '{0} bytes', bytes);
	}
	let value = bytes;
	let unit = 0;
	while (value >= 1000 && unit < units.length - 1) {
		value /= 1000;
		unit++;
	}
	// Three significant digits at most: 4 KB, 38.2 MB, 412 MB.
	const shown = value >= 100 ? Math.round(value).toString() : value.toFixed(1).replace(/\.0$/, '');
	return `${shown} ${units[unit]}`;
}

/** The size column: a file's size, nothing for a folder, since folders have no size of their own. */
export function entrySize(entry: FileEntry): string {
	return entry.kind === 'file' ? formatSize(Number(entry.size)) : '—';
}

/** The modified column: today's time, a date this year, or a full date. */
export function formatModified(mtimeMs: number, now: Date): string {
	if (mtimeMs <= 0) {
		return '';
	}
	const date = new Date(mtimeMs);
	const time = date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	if (date.toDateString() === now.toDateString()) {
		return nls.localize('files.today', 'Today {0}', time);
	}
	if (date.getFullYear() === now.getFullYear()) {
		return `${date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })} ${time}`;
	}
	return date.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

/** A folder's own name: its last part, or its root's name for the root itself. */
export function folderName(folder: FolderLocation, places: FilePlaces | undefined, computer: string): string {
	if (folder.path) {
		return folder.path.slice(folder.path.lastIndexOf('/') + 1);
	}
	const root = places?.roots.find(candidate => candidate.id === folder.root);
	return !root || root.name === '/' ? computer : root.name;
}

/** "Downloads on Studio PC". */
export function folderOn(folder: FolderLocation, places: FilePlaces | undefined, computer: string): string {
	return nls.localize('files.folderOn', '{0} on {1}', folderName(folder, places, computer), computer);
}

/** "3 items", "1 item". */
export function itemCount(count: number): string {
	return count === 1 ? nls.localize('files.oneItem', '1 item') : nls.localize('files.items', '{0} items', count);
}

/** A transfer's progress or outcome in words, short enough for one row. */
export function jobStatus(job: FileJob, rate: number | undefined): string {
	const done = Number(job.done_bytes);
	const total = Number(job.total_bytes);
	switch (job.state) {
		case 'Preparing':
			return nls.localize('files.preparing', 'Checking files…');
		case 'Serving':
		case 'Transferring': {
			if (job.error) {
				return nls.localize('files.retrying', 'Trying again: {0}', job.error);
			}
			if (job.state === 'Serving' && done === 0) {
				return nls.localize('files.waiting', 'Waiting to start');
			}
			const moving = progress(done, total, rate);
			return job.current_file && Number(job.files_total) > 1
				? nls.localize('files.movingFile', '{0} · {1}', baseName(job.current_file), moving)
				: moving;
		}
		case 'WaitingDecision':
			return job.waiting_file
				? nls.localize('files.exists', '“{0}” is already there', baseName(job.waiting_file))
				: nls.localize('files.decide', 'Needs your decision');
		case 'Interrupted':
			if (job.error) {
				return done > 0
					? nls.localize('files.interruptedAtBecause', 'Stopped at {0} of {1}: {2}', formatSize(done), formatSize(total), job.error)
					: nls.localize('files.interruptedBecause', 'Stopped: {0}', job.error);
			}
			return done > 0
				? nls.localize('files.interruptedAt', 'Stopped at {0} of {1}', formatSize(done), formatSize(total))
				: nls.localize('files.interrupted', 'Stopped: the connection was lost');
		case 'Cancelled':
			return nls.localize('files.cancelled', 'Cancelled');
		case 'Failed':
			return job.error ?? nls.localize('files.failed', 'Couldn’t copy');
		case 'Complete':
			return Number(job.files_total) === 1
				? nls.localize('files.completeOne', '{0}, checked', formatSize(total))
				: nls.localize('files.complete', '{0} files, {1}, checked', job.files_total, formatSize(total));
	}
}

/** The last part of a `/`-separated path. */
function baseName(path: string): string {
	return path.slice(path.lastIndexOf('/') + 1);
}

function progress(done: number, total: number, rate: number | undefined): string {
	const amount = nls.localize('files.progress', '{0} of {1}', formatSize(done), formatSize(total));
	if (!rate || rate <= 0 || done >= total) {
		return amount;
	}
	return nls.localize('files.progressRate', '{0} · {1}/s · {2}', amount, formatSize(rate), timeLeft(total - done, rate) ?? '');
}

/** "1 min left", "20 s left". */
function timeLeft(remaining: number, rate: number | undefined): string | undefined {
	if (!rate || rate <= 0 || remaining <= 0) {
		return undefined;
	}
	const seconds = Math.ceil(remaining / rate);
	return seconds < 60
		? nls.localize('files.secondsLeft', '{0} s left', seconds)
		: nls.localize('files.minutesLeft', '{0} min left', Math.ceil(seconds / 60));
}

/** How far a transfer is, 0 to 1. */
export function jobProgress(job: FileJob): number {
	const total = Number(job.total_bytes);
	if (job.state === 'Complete') {
		return 1;
	}
	return total > 0 ? Math.min(1, Number(job.done_bytes) / total) : 0;
}

/**
 * A transfer in the few words its row in the destination folder has room
 * for: "34% · 1 min left", "12 of 86 files", "Stopped at 61%".
 */
export function arrivalStatus(job: FileJob, rate: number | undefined): string {
	const done = Number(job.done_bytes);
	const percent = Math.floor(jobProgress(job) * 100);
	const left = timeLeft(Number(job.total_bytes) - done, rate);
	const files = Number(job.files_total);
	switch (job.state) {
		case 'Preparing':
			return nls.localize('files.preparing', 'Checking files…');
		case 'Serving':
		case 'Transferring': {
			if (job.error) {
				return nls.localize('files.tryingAgain', 'Trying again');
			}
			if (done === 0) {
				return nls.localize('files.waiting', 'Waiting to start');
			}
			const amount = files > 1
				? nls.localize('files.filesOf', '{0} of {1} files', job.files_done, files)
				: nls.localize('files.percent', '{0}%', percent);
			return left ? nls.localize('files.amountLeft', '{0} · {1}', amount, left) : amount;
		}
		case 'WaitingDecision':
			return nls.localize('files.decide', 'Needs your decision');
		case 'Interrupted':
			return done > 0 ? nls.localize('files.stoppedAt', 'Stopped at {0}%', percent) : nls.localize('files.stopped', 'Stopped');
		case 'Cancelled':
			return nls.localize('files.cancelled', 'Cancelled');
		case 'Failed':
			return job.error ?? nls.localize('files.failed', 'Couldn’t copy');
		case 'Complete':
			return nls.localize('files.justNow', 'Just now');
	}
}
