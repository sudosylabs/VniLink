import { colourTolerance, pictureColour, showsStreamColour, shownColour } from '../../src/workbench/services/camera/decoderColour.js';

function assert(condition: boolean, message: string): asserts condition {
	if (!condition) {
		throw new Error(message);
	}
}

/** One codec string per family with an embedded picture, as the window asks about them. */
const codecs = ['avc1.640028', 'hev1.1.6.L153.B0', 'vp8', 'vp09.00.40.08', 'av01.0.08M.08'];

/**
 * Each family's BT.709 picture decodes in software, where this window has a
 * software decoder, to the colour it was made in, so the check that turns
 * down hardware decoders showing another colour measures the right thing.
 * What hardware shows differs between computers and is not asserted.
 */
export async function verifyDecoderColour(): Promise<void> {
	let checked = 0;
	for (const codec of codecs) {
		const software: VideoDecoderConfig = { codec, hardwareAcceleration: 'prefer-software', optimizeForLatency: true };
		if ((await VideoDecoder.isConfigSupported(software)).supported === true) {
			const shown = await shownColour(software);
			assert(shown !== undefined, `${codec}'s picture did not decode in software`);
			assert(shown.every((level, index) => Math.abs(level - pictureColour[index]) <= colourTolerance), `${codec} in software showed ${shown.join(', ')} for ${pictureColour.join(', ')}`);
			assert(await showsStreamColour(software), `${codec} in software was turned down`);
			checked++;
		}
	}
	assert(checked > 0, 'no codec decoded in software');
}
