// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Every `VNILINK_*` environment variable the repository reads or sets, with
 * who sets it and what it means. A tooling test keeps it complete: a name
 * used anywhere in the repository must be here, and a name here must be used.
 *
 * - `setting`: a person sets it, for a build task, a test run or the engine.
 * - `harness`: a VniLink process sets it for one it starts; nobody else needs to.
 * - `manualTest`: an ignored or hardware test asks for it, and stops without it.
 */
export type VariableKind = 'setting' | 'harness' | 'manualTest';

export interface EnvironmentVariable {
	readonly kind: VariableKind;
	readonly description: string;
}

export const environmentVariables: Readonly<Record<string, EnvironmentVariable>> = {
	// The build and its checks.
	VNILINK_MACOS_SIGNING_IDENTITY: { kind: 'setting', description: 'The Developer ID Application identity in the keychain that signs a macOS release, with `VNILINK_MACOS_NOTARY_PROFILE`; unset, the release is unsigned.' },
	VNILINK_MACOS_NOTARY_PROFILE: { kind: 'setting', description: 'The `notarytool` keychain profile that notarizes a signed macOS disk image.' },
	VNILINK_WINDOWS_CERTIFICATE_FILE: { kind: 'setting', description: 'The PKCS#12 code-signing certificate that signs a Windows release and its installer, with `VNILINK_WINDOWS_CERTIFICATE_PASSWORD`; unset, they are unsigned.' },
	VNILINK_WINDOWS_CERTIFICATE_PASSWORD: { kind: 'setting', description: 'The password of `VNILINK_WINDOWS_CERTIFICATE_FILE`.' },
	VNILINK_BUILD_STACK: { kind: 'setting', description: '`1`: a failed build task prints its stack, not only its message.' },
	VNILINK_FORCE_INSTALL: { kind: 'setting', description: '`1`: npm\'s postinstall prepares the first build again although its inputs are unchanged.' },
	VNILINK_MSYS_ROOT: { kind: 'setting', description: 'The MSYS2 root whose `mingw64\\bin` the Windows GNU build uses when MinGW is not on PATH.' },
	VNILINK_SKIP_NODE_VERSION_CHECK: { kind: 'setting', description: '`1`: npm install goes on with a Node version other than `.nvmrc`\'s.' },
	VNILINK_SKIP_PREFLIGHT: { kind: 'setting', description: '`1`: a task starts without checking its prerequisites, for setups the check misjudges.' },
	VNILINK_SKIP_PRELAUNCH: { kind: 'setting', description: '`1`: `dev` and `test:ui` launch without installing or building what is missing or stale.' },
	VNILINK_TEST_CONTAINER: { kind: 'setting', description: '`1` in the Linux test image and CI\'s containers: Electron runs without its sandbox, which a container running as root cannot give it.' },
	VNILINK_HEADLESS_TEST: { kind: 'harness', description: 'Set by `scripts/test.sh` once it runs under its own D-Bus session and Xvfb, so the script does not start them again.' },
	VNILINK_COMMIT: { kind: 'harness', description: 'The checkout\'s commit, which the impaired-network report records; the container has no repository to ask.' },

	// The engine and its logs.
	VNILINK_LOG: { kind: 'setting', description: 'A log filter, in `tracing` syntax, that takes precedence over the application\'s log level.' },
	VNILINK_LOG_DIR: { kind: 'setting', description: 'A folder for the engine\'s log files in place of the one it chooses.' },
	VNILINK_LOG_STDERR: { kind: 'setting', description: '`1`: the engine also writes the events its filter passes, readably, to standard error.' },
	VNILINK_DEV: { kind: 'harness', description: '`1` from `dev`: the Reload Window and Toggle Developer Tools shortcuts, and warnings for events buffered with no listener.' },
	VNILINK_TEST_PROFILE: { kind: 'harness', description: 'The temporary profile a test launch runs in; a smoke or latency launch without one is refused.' },

	// Tests.
	VNILINK_REQUIRE: { kind: 'setting', description: '`all`, or a comma-separated list of the tools and devices a media test needs: a test that would skip without one fails instead.' },
	VNILINK_UNIT_FILES: { kind: 'harness', description: 'The bundled unit test files the Electron unit runner loads, as JSON.' },
	VNILINK_UNIT_GREP: { kind: 'harness', description: 'The pattern of `test:unit:electron --grep`, which selects the tests the runner runs.' },
	VNILINK_UNIT_OUTPUT: { kind: 'harness', description: 'The folder the Electron unit runner\'s bundles were built into.' },
	VNILINK_UNIT_TIMEOUT: { kind: 'harness', description: 'Each unit test\'s timeout in the Electron runner, in milliseconds.' },
	VNILINK_BENCH_DIR: { kind: 'harness', description: 'Where `bench:service` saves each run, to compare with the previous one.' },
	VNILINK_PEER: { kind: 'harness', description: 'The agent file of the computer `--peer` names, which the test kit\'s suites run their other side on.' },
	VNILINK_PEER_CHECK: { kind: 'harness', description: 'The code the agent showed, which proves the agent file is the computer\'s that `--peer` names.' },
	VNILINK_AGENT_NODE: { kind: 'harness', description: 'The Node the test agent starts a measurement\'s side with.' },
	VNILINK_STORE_CHILD: { kind: 'harness', description: 'Which part a storage test\'s child process plays: the test runs itself again to hold a store from another process.' },
	VNILINK_STORE_ENGINE: { kind: 'harness', description: 'The store folder a storage test\'s child process opens.' },
	VNILINK_STORE_KEY: { kind: 'harness', description: 'The key a storage test\'s child process opens the store with.' },
	VNILINK_STORE_SIGNAL: { kind: 'harness', description: 'The file a storage test\'s child process writes once it holds the store.' },
	VNILINK_STORE_CONTINUE: { kind: 'harness', description: 'The file a storage test\'s child process waits for before it lets the store go.' },

	// The latency measurement (`measure:latency`): what a side is given.
	VNILINK_LATENCY_ROLE: { kind: 'harness', description: 'Which side of the measurement this process is: `host` or `viewer`.' },
	VNILINK_LATENCY_DIR: { kind: 'harness', description: 'The folder the two sides meet in, each publishing what the other waits for.' },
	VNILINK_LATENCY_OUTPUT: { kind: 'harness', description: 'Where a side writes its log and results.' },
	VNILINK_LATENCY_ACROSS: { kind: 'harness', description: '`1` when each side runs on its own computer, as a person uses them.' },
	VNILINK_LATENCY_SCREEN_GRANT: { kind: 'harness', description: 'The file that records this computer\'s screen sharing grant for the host side.' },
	VNILINK_LATENCY_HOLD_MS: { kind: 'harness', description: 'How long the stimulus holds still between changes, in milliseconds; 0 when it never does.' },
	VNILINK_LATENCY_FPS: { kind: 'harness', description: 'The viewer\'s frame rate, at the Smooth preset\'s bitrate and size; 0 keeps the default quality.' },
	VNILINK_LATENCY_BITRATE: { kind: 'harness', description: 'The viewer\'s bitrate limit in kb/s, with a frame rate; 0 keeps the Smooth preset\'s.' },
	VNILINK_LATENCY_BUSY: { kind: 'harness', description: '`1`: the stimulus fills the host\'s work area with detail that changes every frame.' },
	VNILINK_LATENCY_CONSENT_MS: { kind: 'harness', description: 'How long someone at the host may take to answer its consent dialog, in milliseconds.' },

	// The impaired-network measurement (`measure:impaired:linux`): what a scenario is given.
	VNILINK_IMPAIRED_HOST_NS: { kind: 'harness', description: 'The network namespace the host side runs in.' },
	VNILINK_IMPAIRED_VIEW_NS: { kind: 'harness', description: 'The network namespace the viewing side runs in.' },
	VNILINK_IMPAIRED_OUT: { kind: 'harness', description: 'The file a scenario writes its result to.' },
	VNILINK_IMPAIRED_SCRATCH: { kind: 'harness', description: 'A folder a scenario may write its files in.' },
	VNILINK_IMPAIRED_RELAY_URL: { kind: 'harness', description: 'The relay both sides connect through, on the relayed path.' },
	VNILINK_IMPAIRED_DRIVER: { kind: 'harness', description: 'How the transfer scenario drives its jobs: `tight` or `background`.' },
	VNILINK_IMPAIRED_SMALL_FILES: { kind: 'harness', description: 'How many small files the transfer scenario sends.' },
	VNILINK_IMPAIRED_SMALL_BYTES: { kind: 'harness', description: 'The size of each small file, in bytes.' },
	VNILINK_IMPAIRED_LARGE_BYTES: { kind: 'harness', description: 'The size of the large file, in bytes.' },
	VNILINK_IMPAIRED_TRANSFER_TIMEOUT_S: { kind: 'harness', description: 'How long a transfer may take before the scenario fails, in seconds.' },
	VNILINK_IMPAIRED_VIEWER: { kind: 'harness', description: 'What the viewer does with a damaged picture: `passive` or `request-while-corrupt`.' },
	VNILINK_IMPAIRED_VIDEO_SECONDS: { kind: 'harness', description: 'How long the video scenario streams, in seconds.' },
	VNILINK_IMPAIRED_BURSTS: { kind: 'harness', description: 'How many bursts of loss the video scenario injects.' },
	VNILINK_IMPAIRED_BURST_MS: { kind: 'harness', description: 'How long each burst of loss lasts, in milliseconds.' },
	VNILINK_IMPAIRED_BURST_START: { kind: 'harness', description: 'The file the video scenario writes when it wants a burst to start.' },
	VNILINK_IMPAIRED_BURST_END: { kind: 'harness', description: 'The file the harness writes when the burst has ended.' },
	VNILINK_IMPAIRED_VIDEO_WARMUP_S: { kind: 'setting', description: 'Seconds the video scenario streams before it measures; 4 by default.' },
	VNILINK_IMPAIRED_STILLS_KBPS: { kind: 'setting', description: 'The bitrate the still-picture scenario streams at, in kb/s; the Smooth preset\'s by default.' },
	VNILINK_IMPAIRED_MIXED_FILES: { kind: 'setting', description: 'How many files of mixed sizes the transfer scenario sends; 200 by default.' },
	VNILINK_IMPAIRED_MIXED_MAX_BYTES: { kind: 'setting', description: 'The largest of the mixed files, in bytes; 4 MiB by default.' },

	// Ignored and hardware tests.
	VNILINK_TEST_X11: { kind: 'harness', description: '`private` when the X11 tests run on an isolated Xvfb server of their own, as `test:rust` runs them on Linux.' },
	VNILINK_TEST_PORTAL_BUS: { kind: 'harness', description: '`private` when the portal test runs on a D-Bus session of its own, as `test:rust` runs it on Linux.' },
	VNILINK_TEST_CAMERA_ID: { kind: 'manualTest', description: 'The camera the Windows camera test opens; none is chosen for it.' },
	VNILINK_AUDIO_CAPTURE_ID: { kind: 'manualTest', description: 'The capture device the audio loopback test records.' },
	VNILINK_AUDIO_OUTPUT_ID: { kind: 'manualTest', description: 'The output device the audio loopback test plays its tone on.' },
	VNILINK_MAC_SYSTEM_AUDIO_ID: { kind: 'manualTest', description: 'The system audio source the macOS audio test captures.' },
	VNILINK_RECORDING_FIXTURE: { kind: 'manualTest', description: 'An absolute path the macOS recording test exports its synthetic recording to.' },
	VNILINK_RECOVERED_FIXTURE: { kind: 'manualTest', description: 'A path the macOS recording test exports its recovered recording to.' },
};

/** Prefixes code reads every variable under, such as the test kit passing `VNILINK_LATENCY_*` on. */
export const environmentPrefixes: readonly string[] = ['VNILINK_LATENCY_', 'VNILINK_UNIT_'];
