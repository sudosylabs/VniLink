// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * How macOS tells VniLink apart from other applications, for the development
 * bundle and the release alike.
 *
 * Local network privacy identifies an application by its main executable's
 * Mach-O UUID (Apple TN3179). Electron ships one executable, and so one UUID,
 * to every application on a release: VniLink's own switch under Local
 * Network did not reach the executable that ran, and its traffic to the
 * computers it paired with on the same network was refused, so every stream
 * went through a relay. Each bundle identifier gets its own UUID instead,
 * derived from the identifier and the stock executable's UUID: a rebuild keeps
 * it, and another identifier (the development bundle) or another Electron
 * changes it.
 */
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';

/**
 * What VniLink asks for on macOS, as each permission prompt says it. Without
 * a local network description macOS refuses that traffic without asking.
 */
export const darwinUsageDescriptions: Readonly<Record<string, string>> = {
	NSCameraUsageDescription: 'Share your camera with computers you have paired and allowed.',
	NSMicrophoneUsageDescription: 'Share your microphone with computers you have paired and allowed.',
	NSAudioCaptureUsageDescription: 'Share this computer’s sound with computers you have paired and allowed.',
	NSLocalNetworkUsageDescription: 'Connect directly to computers you have paired on this network.',
};

const LC_UUID = 0x1b;

/** Where each architecture's LC_UUID sits in a thin or universal Mach-O, and its UUID. */
export function machoUuids(binary: Uint8Array): { readonly offset: number; readonly uuid: Uint8Array }[] {
	const view = new DataView(binary.buffer, binary.byteOffset, binary.byteLength);
	const magic = view.getUint32(0, false);
	if (magic === 0xcafebabe || magic === 0xcafebabf) {
		const wide = magic === 0xcafebabf;
		const count = view.getUint32(4, false);
		const slices: { offset: number; uuid: Uint8Array }[] = [];
		for (let index = 0; index < count; index++) {
			const entry = 8 + index * (wide ? 32 : 20);
			const start = wide ? Number(view.getBigUint64(entry + 8, false)) : view.getUint32(entry + 8, false);
			slices.push(...machoUuids(binary.subarray(start)).map(slice => ({ offset: start + slice.offset, uuid: slice.uuid })));
		}
		return slices;
	}
	const little = view.getUint32(0, true);
	if (little !== 0xfeedfacf && little !== 0xfeedface) {
		throw new Error('Not a Mach-O executable');
	}
	const commands = view.getUint32(16, true);
	let offset = little === 0xfeedfacf ? 32 : 28;
	for (let index = 0; index < commands; index++) {
		const command = view.getUint32(offset, true);
		if (command === LC_UUID) {
			return [{ offset: offset + 8, uuid: binary.slice(offset + 8, offset + 24) }];
		}
		offset += view.getUint32(offset + 4, true);
	}
	throw new Error('The Mach-O executable has no LC_UUID');
}

/** The UUID one architecture of `bundleIdentifier` gets for a stock UUID: an RFC 9562 version 8 UUID. */
export function derivedUuid(bundleIdentifier: string, stock: Uint8Array): Uint8Array {
	const uuid = createHash('sha256').update(bundleIdentifier).update('\0').update(stock).digest().subarray(0, 16);
	uuid[6] = (uuid[6] & 0x0f) | 0x80;
	uuid[8] = (uuid[8] & 0x3f) | 0x80;
	return uuid;
}

export function formatUuid(uuid: Uint8Array): string {
	const hex = Buffer.from(uuid).toString('hex').toUpperCase();
	return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/**
 * The UUIDs the stock executable at `path` gets for `bundleIdentifier`, one
 * per architecture, without changing it.
 */
export async function executableUuids(path: string, bundleIdentifier: string): Promise<string[]> {
	return machoUuids(await readFile(path)).map(slice => formatUuid(derivedUuid(bundleIdentifier, slice.uuid)));
}

/**
 * Gives the stock executable at `path` the UUIDs of `bundleIdentifier`, in
 * place. Its code signature no longer matches: the caller signs it again.
 */
export async function assignExecutableUuids(path: string, bundleIdentifier: string): Promise<string[]> {
	const binary = await readFile(path);
	const assigned = machoUuids(binary).map(slice => {
		const uuid = derivedUuid(bundleIdentifier, slice.uuid);
		binary.set(uuid, slice.offset);
		return formatUuid(uuid);
	});
	await writeFile(path, binary);
	return assigned;
}
