/**
 * Which matrix a WebCodecs decoder shows pictures in correctly.
 *
 * Every VniLink stream is limited range and declares its matrix: BT.709, or
 * BT.601 when the viewer's window asked for it. Chromium 152 on Linux
 * decodes in hardware through VA-API and hands back RGB frames it converted
 * as BT.601, whatever the stream and the frame's `colorSpace` say: a BT.709
 * mid tone comes out 8 levels off and green 20. Drawing cannot undo it,
 * since saturated colours are already clipped; but a stream encoded in
 * BT.601 comes out exact. So a hardware decoder counts once it shows a
 * known picture as it was sent, in BT.709 first and then in BT.601, and the
 * window tells the host which one ({@link hardwareMatrix}); a decoder that
 * shows neither is left to software. A Chromium that converts correctly
 * passes in BT.709 and keeps it.
 */

/** The matrix a stream is converted and declared in. */
export type StreamMatrix = 'bt709' | 'bt601';

/** In the order they are tried: BT.709 is the default every other decoder shows. */
const streamMatrices: readonly StreamMatrix[] = ['bt709', 'bt601'];

type Family = 'h264' | 'h265' | 'vp8' | 'vp9' | 'av1';

interface Picture {
	readonly codec: string;
	readonly data: string;
}

/**
 * One flat 256x256 keyframe per codec family and matrix, BGR 200, 120, 40
 * converted in that matrix, limited range, made by VniLink's own encoders
 * (`crates/vnilink-macos/examples/colour_pictures.rs` makes them again):
 * NVENC for the BT.709 H.264, VideoToolbox for the BT.601 one, Kvazaar,
 * libvpx and libaom for the others. H.264, H.265, VP9 and AV1 declare the
 * matrix; VP8 cannot, so a decoder that follows streams shows its BT.601
 * picture as BT.709.
 */
const pictures: Readonly<Record<StreamMatrix, Readonly<Record<Family, Picture>>>> = {
	bt709: {
		h264: { codec: 'avc1.640028', data: 'AAAAAWdkAA2sKyAgENgLUBAQFAAAD6AAA6mCEAAAAAFo648sAAAAAWW4BB//9/Xin/4RRSv93c0dm8znBCb08JH8ExDxlQzP+Y/8o8EAPEEl4cfRmvD/aR+pkaAGigAnEXaAQAAAAwAAAwAAAwHP' },
		h265: { codec: 'hev1.1.6.L153.B0', data: 'AAAAAUABDAL//wFgAAADAIAAAAMAAAMAugAALAkAAAABQgECAWAAAAMAgAAAAwAAAwC6AACgCAgEBcuSTCpqAgICCAAAH0AAA6mAQAAAAAFEAcBiTwUyQAAAAU4BBaYy/kZsmEFCaa41apFUnvPxS3ZhemFhciBIRVZDIEVuY29kZXIgdi4gMi4zLjIgLSBDb3B5bGVmdCAyMDEyLTIwMTUgLSBodHRwOi8vdWx0cmF2aWRlby5jcy50dXQuZmkvIC0gb3B0aW9uczogMjU2eDI1NiBkZWJsb2NrPTE6MDowIHNhbz0wIGludHJhX3BlcmlvZD0xMjAgcXA9MjIgcmVmPTEAgAAAASYBrkLApYDN83/rYZg1vUfw5zdasDg06VduBAAg4AAAAwAzYAAAByQAAAFQAYQNAgB/gAAAL+AAAA/gAIA=' },
		vp8: { codec: 'vp8', data: 'MBEAnQEqAAEAATgAABwkDCwsRMwkQAFAAJ1qgP4A/oCBralqHpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrK4A/vOk//+j5FPINT/9Hy//zQXhJbr//MvA' },
		vp9: { codec: 'vp09.00.40.08', data: 'g0mDQkAP8A/wAOCQcGAAAANAf0fR+x3FI6Dr2jnAAGtCyBh+3+kiVle8Asu/30y5wOmZrXcmgT/7ik/DAn/3FJ+GBP/uKT8DgA==' },
		av1: { codec: 'av01.0.08M.08', data: 'EgAKDgAAAAO///ml8iAgICCAMi8UACAIAAAhAAAAgLRJBrPP8MhPOPO55PvBSzoTIpf4pb5wu2KDa5prgTXlCqUaCA==' },
	},
	bt601: {
		h264: { codec: 'avc1.640028', data: 'AAAAASdkABWsExQRBAIaagICDA8IBCEYAAAAASjuPLAAAAABJbgAQACFf7VAa50RzEm26heMuZK+H7rWRLTgxIgguSrN4fd1SjyFwPwZPsp+mjovwAAA8wMv+KaoQ3q+8A8I' },
		h265: { codec: 'hev1.1.6.L153.B0', data: 'AAAAAUABDAL//wFgAAADAIAAAAMAAAMAugAALAkAAAABQgECAWAAAAMAgAAAAwAAAwC6AACgCAgEBcuSTCpqAgIMCAAAH0AAA6mAQAAAAAFEAcBiTwUyQAAAAU4BBaYy/kZsmEFCaa41apFUnvPxS3ZhemFhciBIRVZDIEVuY29kZXIgdi4gMi4zLjIgLSBDb3B5bGVmdCAyMDEyLTIwMTUgLSBodHRwOi8vdWx0cmF2aWRlby5jcy50dXQuZmkvIC0gb3B0aW9uczogMjU2eDI1NiBkZWJsb2NrPTE6MDowIHNhbz0wIGludHJhX3BlcmlvZD0xMjAgcXA9MjIgcmVmPTEAgAAAASYBrHkLcpZW6//KMdBvHX/wWPOw6xv6tTo86SNv3YC4dt0AFHAAAAMAS8AAAAv4AAABUAGEDQIAf4AAAC/gAAAP4ACA' },
		vp8: { codec: 'vp8', data: 'MBEAnQEqAAEAATgAABwkDCwsRMwkQAFAAJ1qgP4A/oCBralqHpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrLkkLy9LukMlZckheXpd0hkrK4A/vYTX/+v5sRzPS/+v5//6S0V4BGP/0hwAA==' },
		vp9: { codec: 'vp09.00.40.08', data: 'g0mDQiAP8A/wAOCQcGAAAANAf0fR+x3FI6Dr2jnAAGtDAaE7uRzLLkclADsZvUADZCzc9qJP2M/YwTMTsQ+JQmInYc5B+sRObgA=' },
		av1: { codec: 'av01.0.08M.08', data: 'EgAKDgAAAAO///ml8iAgIMCAMikUACAAALRQzQubqXbUXqH6bjIWC0yE5FPIV/AqUmgS4q1W/hN/TOFKwA==' },
	},
};

/** What the pictures show, blue first. */
export const pictureColour = [200, 120, 40] as const;

/** Levels each channel may be off: codec rounding, not a wrong matrix (8 at least). */
export const colourTolerance = 4;

/** Each family's answer per acceleration, asked once per window. */
const answers = new Map<string, Promise<StreamMatrix | undefined>>();

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
 * The matrix `config`'s decoder shows its family's pictures correctly in:
 * BT.709 when it shows that picture as it was sent, else BT.601 when it
 * shows that one, else `undefined`. A decoder that cannot be asked, as for a
 * codec without a picture or one that puts out nothing, is not held
 * against: only a wrong colour is, and it counts as BT.709.
 */
export function hardwareMatrix(config: VideoDecoderConfig): Promise<StreamMatrix | undefined> {
	const family = familyOf(config.codec);
	if (!family) {
		return Promise.resolve('bt709');
	}
	const key = `${family}/${config.hardwareAcceleration ?? 'no-preference'}`;
	let answer = answers.get(key);
	if (!answer) {
		answer = (async () => {
			for (const matrix of streamMatrices) {
				if (await showsMatrix(config, matrix)) {
					return matrix;
				}
			}
			return undefined;
		})();
		answers.set(key, answer);
	}
	return answer;
}

/** Whether `config`'s decoder shows the picture in `matrix` as it was sent. */
export function showsMatrix(config: VideoDecoderConfig, matrix: StreamMatrix): Promise<boolean> {
	return shownColour(config, matrix).then(
		shown => !shown || shown.every((level, index) => Math.abs(level - pictureColour[index]) <= colourTolerance),
		() => true,
	);
}

/**
 * The colour, blue first, a canvas shows of `config`'s family picture in
 * `matrix` decoded with its acceleration; `undefined` without a picture for
 * the family or a decoded frame.
 */
export async function shownColour(config: VideoDecoderConfig, matrix: StreamMatrix = 'bt709'): Promise<readonly number[] | undefined> {
	const family = familyOf(config.codec);
	if (!family) {
		return undefined;
	}
	const picture = pictures[matrix][family];
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
