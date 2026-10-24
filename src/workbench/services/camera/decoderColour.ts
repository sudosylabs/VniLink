/**
 * Whether a WebCodecs decoder shows pictures in the colour their stream
 * declares.
 *
 * Every VniLink stream is BT.709 in limited range and says so. Chromium 152
 * on Linux decodes in hardware through VA-API and hands back RGB frames it
 * converted as BT.601, whatever the stream and the frame's `colorSpace` say:
 * a mid tone comes out 8 levels off and green 20. Drawing cannot undo it,
 * since saturated colours are already clipped. So a hardware decoder counts
 * only once it shows a known BT.709 picture as it was sent; where it does
 * not, the window decodes in software. A Chromium that converts correctly
 * passes, and keeps its hardware decoder.
 */

type Family = 'h264' | 'h265' | 'vp8' | 'vp9' | 'av1';

/**
 * One flat 256x256 keyframe per codec family, BGR 200, 120, 40 converted as
 * BT.709 limited range, made by VniLink's own encoders: NVENC for H.264,
 * Kvazaar, libvpx and libaom for the others. H.264, H.265, VP9 and AV1
 * declare the colour; VP8 cannot.
 */
const pictures: Readonly<Record<Family, { readonly codec: string; readonly data: string }>> = {
	h264: { codec: 'avc1.640028', data: 'AAAAAWdkAA2sKyAgENgLUBAQFAAAD6AAA6mCEAAAAAFo648sAAAAAWW4BB//9/Xin/4RRSv93c0dm8znBCb08JH8ExDxlQzP+Y/8o8EAPEEl4cfRmvD/aR+pkaAGigAnEXaAQAAAAwAAAwAAAwHP' },
	h265: { codec: 'hev1.1.6.L153.B0', data: 'AAAAAUABDAL//wFgAAADAIAAAAMAAAMAugAALAkAAAABQgECAWAAAAMAgAAAAwAAAwC6AACgCAgEBcuSTCpqAgICCAAAH0AAA6mAQAAAAAFEAcBiTwUyQAAAAU4BBaYy/kZsmEFCaa41apFUnvPxS3ZhemFhciBIRVZDIEVuY29kZXIgdi4gMi4zLjIgLSBDb3B5bGVmdCAyMDEyLTIwMTUgLSBodHRwOi8vdWx0cmF2aWRlby5jcy50dXQuZmkvIC0gb3B0aW9uczogMjU2eDI1NiBkZWJsb2NrPTE6MDowIHNhbz0wIGludHJhX3BlcmlvZD0xMjAgcXA9MjIgcmVmPTEAgAAAASYBrkLApYDN83/rYZg1vUfw5zdasDg06VduBAAg4AAAAwAzYAAAByQAAAFQAYQNAgB/gAAAL+AAAA/gAIA=' },
	vp8: { codec: 'vp8', data: 'MBEAnQEqAAEAATgAABwkDCwsRMwkQAFAAJ1qgP4A/oCBralqHpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrK4A/vOk//+j5FPINT/9Hy//zQXhJbr//MvA' },
	vp9: { codec: 'vp09.00.40.08', data: 'g0mDQkAP8A/wAOCQcGAAAANAf0fR+x3FI6Dr2jnAAGtCyBh+3+kiVle8Asu/30y5wOmZrXcmgT/7ik/DAn/3FJ+GBP/uKT8DgA==' },
	av1: { codec: 'av01.0.08M.08', data: 'EgAKDgAAAAO///ml8iAgICCAMi8UACAIAAAhAAAAgLRJBrPP8MhPOPO55PvBSzoTIpf4pb5wu2KDa5prgTXlCqUaCA==' },
};

/** What the pictures show, blue first. */
export const pictureColour = [200, 120, 40] as const;

/** Levels each channel may be off: codec rounding, not a wrong matrix (8 at least). */
export const colourTolerance = 4;

/** Each family's answer per acceleration, asked once per window. */
const answers = new Map<string, Promise<boolean>>();

function familyOf(codec: string): Family | undefined {
	switch (codec.split('.')[0]) {
		case 'avc1':
		case 'avc3':
			return 'h264';
		case 'hev1':
		case 'hvc1':
			return 'h265';
		case 'vp8':
			return 'vp8';
		case 'vp09':
			return 'vp9';
		case 'av01':
			return 'av1';
		default:
			return undefined;
	}
}

/**
 * Whether `config`'s decoder shows its family's picture as it was sent. A
 * decoder that cannot be asked, as for a codec without a picture or one
 * that puts out nothing, is not held against: only a wrong colour is.
 */
export function showsStreamColour(config: VideoDecoderConfig): Promise<boolean> {
	const family = familyOf(config.codec);
	if (!family) {
		return Promise.resolve(true);
	}
	const key = `${family}/${config.hardwareAcceleration ?? 'no-preference'}`;
	let answer = answers.get(key);
	if (!answer) {
		answer = shownColour(config).then(
			shown => !shown || shown.every((level, index) => Math.abs(level - pictureColour[index]) <= colourTolerance),
			() => true,
		);
		answers.set(key, answer);
	}
	return answer;
}

/**
 * The colour, blue first, a canvas shows of `config`'s family picture
 * decoded with its acceleration; `undefined` without a picture for the
 * family or a decoded frame.
 */
export async function shownColour(config: VideoDecoderConfig): Promise<readonly number[] | undefined> {
	const family = familyOf(config.codec);
	if (!family) {
		return undefined;
	}
	const picture = pictures[family];
	let frame: VideoFrame | undefined;
	const decoder = new VideoDecoder({
		output: decoded => {
			frame?.close();
			frame = decoded;
		},
		error: () => {
			// The flush below rejects with the same error.
		},
	});
	try {
		decoder.configure({ codec: picture.codec, hardwareAcceleration: config.hardwareAcceleration, optimizeForLatency: true });
		const data = Uint8Array.from(atob(picture.data), character => character.charCodeAt(0));
		decoder.decode(new EncodedVideoChunk({ type: 'key', timestamp: 0, data }));
		await decoder.flush();
		if (!frame) {
			return undefined;
		}
		// Read as the views draw: a 2D canvas, the frame drawn at its size.
		const canvas = new OffscreenCanvas(frame.displayWidth, frame.displayHeight);
		const context = canvas.getContext('2d');
		if (!context) {
			return undefined;
		}
		context.drawImage(frame, 0, 0);
		const [red, green, blue] = context.getImageData(canvas.width >> 1, canvas.height >> 1, 1, 1).data;
		return [blue, green, red];
	} finally {
		frame?.close();
		if (decoder.state !== 'closed') {
			decoder.close();
		}
	}
}
