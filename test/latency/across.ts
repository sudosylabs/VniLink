/**
 * What a latency host takes from the rendezvous folder, and what it shares,
 * when the other side is on another computer (`VNILINK_LATENCY_ACROSS=1`).
 * There the folder is filled from the other computer, through the test
 * agent, so the host runs no program named in it and shares its screen to
 * be seen only: the viewer measures pictures, and control is never needed.
 */

/** What the runner's `read` asks of the host. */
export interface ReadRequest {
	/** How long to sample, 1..600 s. */
	readonly seconds: number;
	/** On one computer only: the reader the runner built, its samples file and its grant. */
	readonly reader: { readonly path: string; readonly samples: string; readonly grant: string } | undefined;
}

/** `read`'s fields as a request; across two computers a reader is refused, as is anything but whole seconds. */
export function readRequest(value: Readonly<Record<string, unknown>>, across: boolean): ReadRequest {
	const seconds = value.seconds;
	if (typeof seconds !== 'number' || !Number.isInteger(seconds) || seconds < 1 || seconds > 600) {
		throw new Error('read.seconds must be a whole number of seconds, 1..600');
	}
	const named = ['reader', 'samples', 'grant'].filter(field => value[field] !== undefined);
	if (across) {
		if (named.length > 0) {
			throw new Error(`Across two computers the host runs no reader; read named ${named.join(', ')}`);
		}
		return { seconds, reader: undefined };
	}
	if (named.length === 0) {
		return { seconds, reader: undefined };
	}
	const { reader, samples, grant } = value;
	if (typeof reader !== 'string' || typeof samples !== 'string' || typeof grant !== 'string') {
		throw new Error('read names its reader, samples and grant together');
	}
	return { seconds, reader: { path: reader, samples, grant } };
}

/** What a host across two computers shares once screen sharing is on: seen, not controlled. */
export const ACROSS_SHARING = { view: true, control: false } as const;

/** Throws unless `policy`, the engine's sharing policy, lets the screen be seen and not controlled. */
export function assertSeenOnly(policy: unknown): void {
	const fields = typeof policy === 'object' && policy !== null ? policy as Record<string, unknown> : {};
	if (fields.view !== true || fields.control !== false) {
		throw new Error('The host across two computers must share its screen to be seen and not controlled');
	}
}
