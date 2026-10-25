/** Commands on another computer over SSH, for measurements that run one side there. */
import { execFile } from 'node:child_process';
import { join } from 'node:path';
import { promisify } from 'node:util';

/** Another computer reached over SSH. */
export interface SshTarget {
	/** The SSH destination, as `ssh` takes it. */
	readonly destination: string;
	/** An SSH configuration file for every connection, or the user's own. */
	readonly sshConfig: string | undefined;
}

/** A POSIX shell word: the remote side's commands go through its login shell. */
export function shellWord(value: string): string {
	return `'${value.replaceAll('\'', '\'\\\'\'')}'`;
}

/** Runs commands on the remote computer over one shared SSH connection. */
export interface RemoteShell {
	/** `ssh`'s arguments up to and including the destination. */
	readonly args: readonly string[];
	/** Runs a shell command there and returns its output. */
	output(command: string): Promise<string>;
	/** Closes the shared connection. */
	close(): Promise<void>;
}

/** One shared connection for the run, its control socket in `work`. */
export function remoteShell(target: SshTarget, work: string, signal: AbortSignal): RemoteShell {
	const args = [
		...(target.sshConfig ? ['-F', target.sshConfig] : []),
		'-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', '-o', 'ServerAliveInterval=15',
		'-o', 'ControlMaster=auto', '-o', `ControlPath=${join(work, 'ssh-%h-%p')}`, '-o', 'ControlPersist=120',
		target.destination,
	];
	return {
		args,
		output: async command => (await promisify(execFile)('ssh', [...args, command], { signal, maxBuffer: 64 * 1024 * 1024 })).stdout,
		close: async () => {
			await promisify(execFile)('ssh', [...args.slice(0, -1), '-O', 'exit', target.destination]).catch(() => undefined);
		},
	};
}
