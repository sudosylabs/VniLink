/** NAL unit type of a sequence parameter set. */
const SPS = 7;

function hex(byte: number): string {
	return byte.toString(16).toUpperCase().padStart(2, '0');
}

/**
 * The WebCodecs codec string (`avc1.PPCCLL`) of an Annex-B H.264 access unit,
 * read from its sequence parameter set: profile, constraint flags and level.
 * `undefined` when the unit carries no complete SPS, as a predicted frame
 * does.
 */
export function avcCodec(data: Uint8Array): string | undefined {
	for (let index = 0; index + 3 < data.length; index++) {
		// A start code is 00 00 01; the four-byte form 00 00 00 01 ends the same way.
		if (data[index] !== 0 || data[index + 1] !== 0 || data[index + 2] !== 1) {
			continue;
		}
		const header = index + 3;
		if ((data[header] & 0x1f) === SPS && header + 3 < data.length) {
			return `avc1.${hex(data[header + 1])}${hex(data[header + 2])}${hex(data[header + 3])}`;
		}
		index = header - 1;
	}
	return undefined;
}
