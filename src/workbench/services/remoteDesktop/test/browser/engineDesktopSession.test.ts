// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../../base/common/async.js';
import { Emitter } from '../../../../../base/common/event.js';
import { DisposableStore } from '../../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import type { DataPacket, DesktopCursor, DesktopDecoderReport, DesktopMediaStatus } from '../../../../../platform/engine/common/dataChannel.js';
import type { DesktopInputCommand } from '../../../../../platform/engine/common/engine.js';
import type { CameraDecoder, CameraDecoderFactory } from '../../../camera/engineCameraStream.js';
import type { DesktopLatencySummary } from '../../desktopLatency.js';
import { desktopCodec, EngineDesktopSession, pointerShape, type DesktopVideoChannel } from '../../engineDesktopSession.js';
import type { DesktopEnd, DesktopPicture, DesktopPointer } from '../../remoteDesktopService.js';

/** An H.264 keyframe's start: a start code, then an SPS with profile 0x64, flags 0x00, level 0x28. */
const keyframe = new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac]);

const stableStatus: DesktopMediaStatus = { reason: 'stable', receivedKbps: 1, receivedFps: 1, receiverQueueMs: 0, hostQueueMs: 0, lossPerMille: 0, rttMs: 20, encodeUs: 0, captureUs: 0, limit: 'none', conditions: [] };

/** A one-pixel picture with `timestamp`, as a decoder puts it out. */
function videoFrame(timestamp: number): VideoFrame {
	return new VideoFrame(new Uint8Array(4), { format: 'RGBA', codedWidth: 1, codedHeight: 1, timestamp });
}

/** A video packet with the metadata of a changed H.264 keyframe, `fields` replacing any of it. */
function videoPacket(fields: Record<string, unknown> = {}, data = keyframe): DataPacket {
	return { type: 'data', sequence: 1, kind: 'video', data, metadata: JSON.stringify({ codec: 'h264', keyframe: true, timestampUs: '1000', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0, role: 'changed', ...fields }) };
}

/** The channel's last packet, its metadata `fields`. */
function exitPacket(fields: Record<string, unknown>): DataPacket {
	return { type: 'data', sequence: 1, kind: 'exited', metadata: JSON.stringify(fields) };
}

/** What a session's stats say of where its pictures' time went. */
async function latencyOf(session: EngineDesktopSession): Promise<DesktopLatencySummary> {
	const stats = await session.stats();
	assert.ok(stats, 'the session had no stats');
	return stats.latency;
}

suite('EngineDesktopSession', () => {
	const disposables = new DisposableStore();
	/** Frames handed to listeners belong to them; the suite closes them. */
	const shown: VideoFrame[] = [];

	/** A picture channel with no pointer, keyframe or sound of its own, reporting each decoder report to `mediaStatus`. */
	function videoChannel(mediaStatus: DesktopVideoChannel['mediaStatus'], overrides: Partial<DesktopVideoChannel> = {}): DesktopVideoChannel {
		return {
			onCursor: disposables.add(new Emitter<DesktopCursor>()).event,
			requestKeyframe: () => undefined,
			mediaStatus,
			desktopAudio: () => Promise.reject(new Error('This test plays no sound')),
			close: () => undefined,
			...overrides,
		};
	}

	/** A session whose decoders output each picture at once, recording the codec asked for, what reached them and what the session reported. */
	function desktop({ control = true, decodes = true } = {}) {
		const codecs: string[] = [];
		const closed: string[] = [];
		const failures: (() => void)[] = [];
		const createDecoder: CameraDecoderFactory = (codec, output, error) => {
			codecs.push(codec);
			failures.push(error);
			if (!decodes) {
				return Promise.resolve(undefined);
			}
			return Promise.resolve({
				software: false,
				decode: chunk => {
					output(videoFrame(chunk.timestamp));
				},
				drained: () => Promise.resolve(),
				close: () => closed.push(codec),
			});
		};
		const session = disposables.add(new EngineDesktopSession(control, { id: '4', generation: '2' }, createDecoder, () => Promise.resolve([])));
		const reports: DesktopDecoderReport[] = [];
		const inputs: DesktopInputCommand[] = [];
		const keyframes: true[] = [];
		const cursors = disposables.add(new Emitter<DesktopCursor>());
		session.attach({
			video: videoChannel(decoder => {
				reports.push(decoder);
				return Promise.resolve(stableStatus);
			}, { onCursor: cursors.event, requestKeyframe: () => keyframes.push(true) }),
			input: control ? { input: command => inputs.push(command), close: () => undefined } : undefined,
		});
		const pictures: DesktopPicture[] = [];
		const ends: DesktopEnd[] = [];
		const pointers: DesktopPointer[] = [];
		disposables.add(session.onPicture(picture => {
			shown.push(picture.frame);
			pictures.push(picture);
		}));
		disposables.add(session.onEnd(end => ends.push(end)));
		disposables.add(session.onPointer(pointer => pointers.push(pointer)));
		return { session, codecs, closed, inputs, pictures, ends, pointers, failures, keyframes, cursors, reports };
	}
	teardown(() => {
		disposables.clear();
		for (const frame of shown.splice(0)) {
			frame.close();
		}
	});
	suite('decoding', () => {
		test('a predicted picture before any keyframe opens no decoder', async () => {
			const { session, codecs } = desktop();
			await session.consume(videoPacket({ keyframe: false }));
			assert.deepStrictEqual(codecs, []);
		});
		test('a picture is decoded with its own codec and keeps the display and revision it was taken on', async () => {
			const { session, codecs, pictures } = desktop();
			await session.consume(videoPacket({ keyframe: false }));
			await session.consume(videoPacket());
			await session.consume(videoPacket({ keyframe: false, timestampUs: '2000', revision: '4' }));
			assert.deepStrictEqual(codecs, ['avc1.640028']);
			assert.deepStrictEqual(pictures.map(picture => [picture.frame.timestamp, picture.display, picture.revision]), [[1000, '7', '3'], [2000, '7', '4']]);
		});
		test('another codec after a reconnect starts a new decoder at its keyframe', async () => {
			const { session, codecs, closed } = desktop();
			await session.consume(videoPacket());
			await session.consume(videoPacket({ codec: 'vp9', keyframe: true, timestampUs: '5000' }, new Uint8Array([1, 2, 3])));
			assert.deepStrictEqual(codecs, ['avc1.640028', 'vp09.00.40.08']);
			assert.deepStrictEqual(closed, ['avc1.640028']);
		});
		for (const [codec, expected] of [['av1', 'av01.0.08M.08'], ['mjpeg', undefined]] as const) {
			test(`the codec ${codec} is decoded as ${expected ?? 'nothing'}`, () => {
				assert.strictEqual(desktopCodec(codec, new Uint8Array()), expected);
			});
		}
		test('a picture this computer cannot decode ends the session once', async () => {
			const { session, ends } = desktop({ decodes: false });
			await session.consume(videoPacket());
			await session.consume(videoPacket());
			assert.deepStrictEqual(ends, ['cantShow']);
		});
		test('a packet whose display is not a string is refused', async () => {
			const { session } = desktop();
			await assert.rejects(session.consume(videoPacket({ displayId: 7 })), /Invalid desktop packet/);
		});
		test('a decoder that loses the picture asks for a keyframe instead of ending the session', async () => {
			const { session, closed, ends, failures, keyframes } = desktop();
			await session.consume(videoPacket());
			failures[failures.length - 1]();
			await timeout(0);
			assert.deepStrictEqual(ends, [], 'a lost picture ended the session');
			assert.deepStrictEqual(keyframes, [true]);
			assert.deepStrictEqual(closed, ['avc1.640028']);
		});
		test('after a lost picture, predicted pictures wait and the next keyframe opens a new decoder', async () => {
			const { session, codecs, failures, pictures } = desktop();
			await session.consume(videoPacket());
			failures[failures.length - 1]();
			await timeout(0);
			await session.consume(videoPacket({ keyframe: false, timestampUs: '2000' }));
			assert.strictEqual(codecs.length, 1);
			await session.consume(videoPacket({ timestampUs: '3000' }));
			assert.strictEqual(codecs.length, 2);
			assert.deepStrictEqual(pictures.map(picture => picture.frame.timestamp), [1000, 3000]);
		});
	});
	suite('input', () => {
		test('input carries the session', () => {
			const { session, inputs } = desktop();
			session.send({ type: 'submitLocalKey', usage: 4, down: true, text: 'a', composing: false });
			assert.deepStrictEqual(inputs, [{ type: 'submitLocalKey', usage: 4, down: true, text: 'a', composing: false, sessionId: '4', generation: '2' }]);
		});
		test('an ended session sends no input', () => {
			const { session, inputs } = desktop();
			session.lost();
			session.send({ type: 'releaseSessionInput' });
			assert.deepStrictEqual(inputs, []);
		});
		test('a view-only session sends no input', () => {
			const { session, inputs } = desktop({ control: false });
			session.send({ type: 'submitLocalKey', usage: 4, down: true, text: 'a', composing: false });
			assert.deepStrictEqual(inputs, []);
		});
	});
	suite('decoder reports', () => {
		test('the engine hears whether this computer decodes in software, and of no recovery before any', async () => {
			const { session, reports } = desktop();
			await session.consume(videoPacket());
			await session.stats();
			assert.deepStrictEqual(reports.at(-1), { decodeUs: 0, software: false, recoveries: 0, stillsShown: 0 });
		});
		test('a decoder restart after a picture is reported as a recovery', async () => {
			const { session, failures, reports } = desktop();
			await session.consume(videoPacket());
			failures[failures.length - 1]();
			await timeout(0);
			await session.consume(videoPacket({ timestampUs: '3000' }));
			await session.stats();
			assert.strictEqual(reports.at(-1)?.recoveries, 1);
		});
		test('a decoder that fell back from hardware says so', async () => {
			const software: CameraDecoderFactory = (_codec, output) => Promise.resolve({
				software: true,
				decode: chunk => {
					output(videoFrame(chunk.timestamp));
				},
				drained: () => Promise.resolve(),
				close: () => undefined,
			});
			const session = disposables.add(new EngineDesktopSession(true, { id: '4', generation: '2' }, software, () => Promise.resolve([])));
			const heard: DesktopDecoderReport[] = [];
			session.attach({
				video: videoChannel(decoder => {
					heard.push(decoder);
					return Promise.resolve(undefined);
				}), input: undefined,
			});
			await session.consume(videoPacket());
			await session.stats();
			assert.strictEqual(heard.at(-1)?.software, true);
		});
		test('a picture alone in the decoder is not taken for load', () => runWithFakedTimers({}, async () => {
			const { session, output, held, reported } = slowDecoder();
			// Each out long after it went in, one at a time: latency, not load.
			for (const [timestampUs, keyframe] of [['1', true], ['2', false]] as const) {
				await session.consume(videoPacket({ timestampUs, keyframe }, keyframe ? undefined : new Uint8Array([1])));
				await timeout(30);
				output(videoFrame(held.splice(0, 1)[0]));
				await session.stats();
			}
			assert.deepStrictEqual(reported, [0, 0]);
		}));
		test('pictures waiting in the decoder report its time per picture', () => runWithFakedTimers({}, async () => {
			const { session, output, held, reported } = slowDecoder();
			await session.consume(videoPacket({ timestampUs: '1' }));
			await timeout(30);
			output(videoFrame(held.splice(0, 1)[0]));
			await session.stats();
			// Three waiting at once, put out 10 ms apart: 10 ms a picture.
			for (const timestamp of ['2', '3', '4']) {
				await session.consume(videoPacket({ timestampUs: timestamp, keyframe: false }, new Uint8Array([1])));
			}
			for (const timestamp of held.splice(0)) {
				await timeout(10);
				output(videoFrame(timestamp));
			}
			await session.stats();
			assert.strictEqual(reported[1], 10_000);
		}));

		/** A session whose decoder holds every picture in `held` until the test puts it out with `output`. */
		function slowDecoder() {
			const held: number[] = [];
			let emit: (frame: VideoFrame) => void = () => {
				throw new Error('No decoder is open');
			};
			const createDecoder: CameraDecoderFactory = (_codec, output) => {
				emit = output;
				return Promise.resolve({
					software: false,
					decode: chunk => held.push(chunk.timestamp),
					drained: () => Promise.resolve(),
					close: () => undefined,
				});
			};
			const reported: number[] = [];
			const session = disposables.add(new EngineDesktopSession(true, { id: '4', generation: '2' }, createDecoder, () => Promise.resolve([])));
			session.attach({
				video: videoChannel(({ decodeUs }) => {
					reported.push(decodeUs);
					return Promise.resolve(stableStatus);
				}), input: undefined,
			});
			return {
				session, held, reported, output: (frame: VideoFrame) => {
					emit(frame);
				},
			};
		}
	});
	suite('pointer', () => {
		/** Premultiplied BGRA: a half-transparent red, and a clear pixel. */
		const image = { width: 2, height: 1, hotspotX: 1, hotspotY: 0, pixels: new Uint8Array([0, 0, 128, 128, 0, 0, 0, 0]) };
		test('a pointer shape is unpremultiplied for a canvas', () => {
			assert.deepStrictEqual([...pointerShape(image).rgba], [255, 0, 0, 128, 0, 0, 0, 0]);
		});
		test('the other pointer keeps its shape across moves', () => {
			const { cursors, pointers } = desktop();
			cursors.fire({ x: 5, y: 6, visible: true, image });
			cursors.fire({ x: 7, y: 8, visible: false });
			const seen = pointers.map(pointer => [pointer.x, pointer.y, pointer.visible, pointer.shape?.hotspotX]);
			assert.deepStrictEqual(seen, [[5, 6, true, 1], [7, 8, false, 1]]);
			assert.strictEqual(pointers[0].shape, pointers[1].shape, 'a move made the shape again');
		});
		test('an ended session no longer reports the pointer', () => {
			const { session, cursors, pointers } = desktop();
			session.lost();
			cursors.fire({ x: 9, y: 9, visible: true });
			assert.deepStrictEqual(pointers, []);
		});
	});
	suite('latency', () => {
		test('a picture reports where its time went, from the other computer to the screen', async () => {
			const { session, pictures } = desktop();
			await session.consume({ ...videoPacket({ hostUs: 12_000, queuedUs: 3_000 }), readAt: performance.timeOrigin + performance.now() });
			assert.strictEqual(pictures.length, 1);
			session.presented(pictures[0]);
			const latency = await latencyOf(session);
			assert.deepStrictEqual(Object.keys(latency).sort(), ['bridge', 'decode', 'draw', 'host', 'queue']);
			assert.strictEqual(latency.host?.p50, 12);
			assert.strictEqual(latency.queue?.p50, 3);
			for (const stage of ['bridge', 'decode', 'draw'] as const) {
				assert.ok((latency[stage]?.p50 ?? -1) >= 0, `the ${stage} stage took no time`);
			}
		});
		test('a picture sent without its capture time measures no host stage', async () => {
			const { session } = desktop();
			await session.consume(videoPacket({ hostUs: 12_000 }));
			await session.stats();
			await session.consume(videoPacket({ timestampUs: '2000', keyframe: false }));
			assert.strictEqual((await latencyOf(session)).host, undefined);
		});
		for (const [field, value] of [['hostUs', -1], ['queuedUs', '5']] as const) {
			test(`a packet with ${field} ${JSON.stringify(value)} is refused`, async () => {
				const { session } = desktop();
				await assert.rejects(session.consume(videoPacket({ [field]: value })), /Invalid desktop packet/);
			});
		}
	});
	suite('end', () => {
		for (const end of ['hostEncoderFailed', 'hostPermission', 'relayUnreachable', 'peerClosed', 'revoked'] as const) {
			test(`a last packet saying ${end} ends the picture for that reason, which a later loss does not replace`, async () => {
				const { session, ends } = desktop();
				await session.consume(exitPacket({ end }));
				session.lost();
				assert.deepStrictEqual(ends, [end]);
			});
		}
		for (const [name, fields] of [['a reason the engine does not send', { end: 'cantShow' }], ['an extra field', { end: 'linkLost', extra: 1 }]] as const) {
			test(`a last packet with ${name} is refused`, async () => {
				const { session } = desktop();
				await assert.rejects(session.consume(exitPacket(fields)), /Invalid desktop end/);
			});
		}
		test('a channel that fails without a reason is a lost connection', () => {
			const { session, ends } = desktop();
			session.lost();
			assert.deepStrictEqual(ends, ['linkLost']);
		});
	});
	suite('stills', () => {
		test('a still picture has a decoder of its own, and the stream\'s decoder survives it', async () => {
			const { session, codecs, closed, pictures } = desktop();
			await session.consume(videoPacket());
			await session.consume(videoPacket({ codec: 'av1', role: 'still', timestampUs: '1500' }, new Uint8Array([1, 2])));
			await session.consume(videoPacket({ keyframe: false, timestampUs: '2000', role: 'unchanged' }));
			assert.deepStrictEqual(codecs, ['avc1.640028', 'av01.1.08M.08.0.000']);
			assert.deepStrictEqual(closed, [], 'a still closed a decoder');
			assert.deepStrictEqual(pictures.map(picture => [picture.frame.timestamp, picture.role, picture.display]), [[1000, 'changed', '7'], [1500, 'still', '7'], [2000, 'unchanged', '7']]);
		});
		test('the engine hears of each still shown', async () => {
			const { session, reports } = desktop();
			await session.consume(videoPacket());
			await session.consume(videoPacket({ codec: 'av1', role: 'still', timestampUs: '1500' }, new Uint8Array([1, 2])));
			await session.stats();
			assert.strictEqual(reports.at(-1)?.stillsShown, 1);
		});
		for (const role of ['refined', undefined]) {
			test(`a picture with the role ${role ?? 'missing'} is refused`, async () => {
				const { session } = desktop();
				await assert.rejects(session.consume(videoPacket({ role })), /Invalid desktop packet/);
			});
		}

		/** A view-only session whose still decoder holds each still until `release`, and fails at `fail`. */
		function heldStills() {
			let release = (): void => {
				throw new Error('No still is waiting');
			};
			let fail = (): void => {
				throw new Error('No still decoder is open');
			};
			const createDecoder: CameraDecoderFactory = (codec, output, error) => {
				const still = codec.startsWith('av01.1');
				if (still) {
					fail = error;
				}
				const decoder: CameraDecoder = {
					software: true,
					decode: chunk => {
						const frame = videoFrame(chunk.timestamp);
						if (still) {
							release = () => {
								output(frame);
							};
						} else {
							output(frame);
						}
					},
					drained: () => Promise.resolve(),
					close: () => undefined,
				};
				return Promise.resolve(decoder);
			};
			const session = disposables.add(new EngineDesktopSession(false, { id: '4', generation: '2' }, createDecoder, () => Promise.resolve([])));
			session.attach({ video: videoChannel(() => Promise.resolve(undefined)), input: undefined });
			const pictures: DesktopPicture[] = [];
			const ends: DesktopEnd[] = [];
			disposables.add(session.onPicture(picture => {
				shown.push(picture.frame);
				pictures.push(picture);
			}));
			disposables.add(session.onEnd(end => ends.push(end)));
			const packet = (role: 'changed' | 'still', timestampUs: string) => videoPacket({ codec: role === 'still' ? 'av1' : 'h264', timestampUs, role }, role === 'still' ? new Uint8Array([1]) : keyframe);
			return {
				session, pictures, ends, packet,
				release: () => {
					release();
				},
				fail: () => {
					fail();
				},
			};
		}
		test('a still decoded after the screen changed is not shown', async () => {
			const { session, pictures, packet, release } = heldStills();
			await session.consume(packet('changed', '1'));
			await session.consume(packet('still', '2'));
			await session.consume(packet('changed', '3'));
			release();
			assert.deepStrictEqual(pictures.map(picture => picture.role), ['changed', 'changed']);
		});
		test('a still decoded before the screen changes again is shown', async () => {
			const { session, pictures, packet, release } = heldStills();
			await session.consume(packet('changed', '1'));
			await session.consume(packet('still', '2'));
			release();
			assert.deepStrictEqual(pictures.map(picture => picture.role), ['changed', 'still']);
		});
		test('a failing still decoder ends nothing', async () => {
			const { session, ends, packet, fail } = heldStills();
			await session.consume(packet('changed', '1'));
			await session.consume(packet('still', '2'));
			fail();
			await session.consume(packet('still', '3'));
			assert.deepStrictEqual(ends, []);
		});
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});
