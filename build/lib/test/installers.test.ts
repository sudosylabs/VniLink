// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { validateConfiguration } from 'app-builder-lib/out/util/config/config.js';
import { DebugLogger } from 'builder-util';
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { buildInstallers, installerConfiguration, installerExtensions, verifyInstallerFiles, type InstallerOptions } from '../installers.ts';
import { packagedLayout, verifyPackageFiles, type BuilderConfiguration, type PackagedLayout } from '../package.ts';
import { readProduct, type ProductPlatform } from '../product.ts';
import { fileSha256, packageInventory, releaseInventory } from '../inventory.ts';
import { isMacintosh, isWindows } from '../platform.ts';
import type { SigningKind } from '../signing.ts';

const product = await readProduct(fileURLToPath(new URL('../../../', import.meta.url)));

const platforms: readonly ProductPlatform[] = ['darwin', 'win32', 'linux'];

function configuration(platform: ProductPlatform): BuilderConfiguration {
	return installerConfiguration('/repo', '/stage', '/output', product, { platform, electronVersion: '44.7.0' });
}

/** `path` with forward slashes, as on the macOS host that builds the DMG. */
function packagedPath(path: string): string {
	return path.split('\\').join('/');
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** One electron-builder option group, such as `nsis` or `dmg`. */
function options(config: BuilderConfiguration, name: string): Readonly<Record<string, unknown>> {
	const value = config[name];
	assert.ok(isRecord(value), `${name} is configured`);
	return value;
}

/** A checkout with a verified assembled application and its release stage, as `buildInstallers` reads them. */
interface Checkout {
	readonly root: string;
	readonly target: string;
	readonly application: string;
	readonly layout: PackagedLayout;
	readonly output: string;
}

async function checkout(signing: SigningKind = 'unsigned'): Promise<Checkout> {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-installer-contract-'));
	const target = `${process.platform}-${process.arch}`;
	const application = join(root, 'dist', target);
	const layout = packagedLayout(application, process.platform, product);
	await mkdir(join(layout.resources, 'app.asar.unpacked/native'), { recursive: true });
	await writeFile(layout.asar, 'test application archive');
	await writeFile(layout.native, 'test native library');
	const manifest = { appId: product.appId, version: '0.1.0', electronVersion: '44.7.0', platform: process.platform, architecture: process.arch, signing, asarSha256: await fileSha256(layout.asar), nativeSha256: await fileSha256(layout.native), files: await packageInventory(application) };
	await writeFile(join(application, 'packaging-manifest.json'), JSON.stringify(manifest));
	const stage = join(root, '.build/release', target);
	await mkdir(join(stage, 'app'), { recursive: true });
	await writeFile(join(stage, 'app/product.json'), JSON.stringify(product));
	await writeFile(join(stage, 'build-manifest.json'), JSON.stringify({ profile: 'release', version: manifest.version, electronVersion: manifest.electronVersion, platform: process.platform, architecture: process.arch, files: await releaseInventory(stage) }));
	await writeFile(join(root, 'product.json'), JSON.stringify(product));
	return { root, target, application, layout, output: join(root, 'dist/installers', target) };
}

function argument(args: readonly string[], flag: string): string {
	const index = args.indexOf(flag);
	assert.ok(index >= 0 && index + 1 < args.length, `${flag} is passed with a value`);
	return args[index + 1];
}

/** Stands in for electron-builder: writes one artifact per installer extension where the configuration says. */
async function writeArtifacts(args: readonly string[], name: string): Promise<void> {
	const config = JSON.parse(await readFile(argument(args, '--config'), 'utf8')) as { readonly directories: { readonly output: string } };
	await mkdir(config.directories.output);
	for (const extension of installerExtensions(process.platform)) {
		await writeFile(join(config.directories.output, `${name}${extension}`), 'installer artifact');
	}
}

const builder: InstallerOptions = { runBuilder: (_processes, args) => writeArtifacts(args, 'VniLink-test') };

const failingBuilder: InstallerOptions = { runBuilder: () => Promise.reject(new Error('installer tool failed')) };

function never(): AbortSignal {
	return new AbortController().signal;
}

suite('installers', () => {
	for (const platform of platforms) {
		test(`the ${platform} configuration validates against the pinned builder schema`, async () => {
			await validateConfiguration(configuration(platform), new DebugLogger(false));
		});
		test(`${platform === 'win32' ? 'sets' : 'does not set'} NSIS options on ${platform}`, () => {
			assert.strictEqual(configuration(platform).nsis === undefined, platform !== 'win32');
		});
	}
	test('builds a DMG on macOS', () => {
		assert.deepStrictEqual(configuration('darwin').mac.target, ['dmg']);
	});
	test('shows the DMG window over its staged artwork, the application beside the Applications link', () => {
		const dmg = options(configuration('darwin'), 'dmg');
		assert.strictEqual(packagedPath(String(dmg.background)), '/stage/packaging-resources/assets/apple/dmg/background.png');
		assert.deepStrictEqual(dmg.contents, [{ x: 140, y: 170, type: 'file' }, { x: 400, y: 170, type: 'link', path: '/Applications' }]);
	});
	test('leaves the DMG unsigned', () => {
		assert.strictEqual(options(configuration('darwin'), 'dmg').sign, false);
	});
	test('builds an NSIS installer on Windows', () => {
		assert.deepStrictEqual(configuration('win32').win.target, ['nsis']);
	});
	test('installs for the current user only on Windows', () => {
		assert.strictEqual(options(configuration('win32'), 'nsis').perMachine, false);
	});
	test('keeps user data when VniLink is uninstalled from Windows', () => {
		assert.strictEqual(options(configuration('win32'), 'nsis').deleteAppDataOnUninstall, false);
	});
	test('the Windows installer is assisted: the person sees its pages and chooses the folder', () => {
		const nsis = options(configuration('win32'), 'nsis');
		assert.deepStrictEqual([nsis.oneClick, nsis.allowToChangeInstallationDirectory, nsis.allowElevation], [false, true, false]);
	});
	test('the Windows uninstaller clears the invitation extension package.ts registers', async () => {
		const script = await readFile(new URL('../windows/installer.nsh', import.meta.url), 'utf8');
		const { ext, name } = (configuration('win32').fileAssociations as readonly { readonly ext: string; readonly name: string }[])[0];
		assert.ok(script.includes(`"Software\\Classes\\.${ext}" ""`) && script.includes(`$0 == "${name}"`), 'installer.nsh names another extension or file type');
	});
	test('the Windows installer offers to start VniLink when it finishes', () => {
		assert.strictEqual(options(configuration('win32'), 'nsis').runAfterFinish, true);
	});
	test('the Windows installer shows the staged license, artwork and its own pages', () => {
		const nsis = options(configuration('win32'), 'nsis');
		const staged = (path: unknown) => packagedPath(String(path)).replace('/stage/packaging-resources/', '');
		assert.deepStrictEqual([nsis.license, nsis.include, nsis.installerSidebar, nsis.installerHeader].map(staged), [
			'LICENSE', 'build/lib/windows/installer.nsh', 'assets/windows/installer-sidebar.bmp', 'assets/windows/installer-header.bmp',
		]);
	});
	test('builds deb and rpm packages on Linux', () => {
		assert.deepStrictEqual(configuration('linux').linux.target, ['deb', 'rpm']);
	});
	test('names the Linux desktop file after the application', () => {
		assert.strictEqual(configuration('linux').linux.syncDesktopName, true);
	});
	test('refuses an unsupported installer host', () => {
		assert.throws(() => installerExtensions('aix'), /Unsupported/);
	});
	suite('with an assembled application', () => {
		let f: Checkout;
		beforeEach(async () => {
			f = await checkout();
		});
		afterEach(async () => {
			await rm(f.root, { recursive: true, force: true });
		});
		test('package verification accepts the application as assembled', async () => {
			await verifyPackageFiles(f.application, product);
		});
		test('package verification rejects an added resource', async () => {
			await writeFile(join(f.layout.resources, 'unexpected'), 'extra');
			await assert.rejects(verifyPackageFiles(f.application, product), /inventory differs/);
		});
		test('package verification rejects a modified native library', async () => {
			await writeFile(f.layout.native, 'changed native library');
			await assert.rejects(verifyPackageFiles(f.application, product), /inventory differs/);
		});
		test('package verification rejects an omitted native library', async () => {
			await rm(f.layout.native);
			await assert.rejects(verifyPackageFiles(f.application, product), /inventory differs/);
		});
		const links = { skip: process.platform === 'win32' };
		test('the inventory records a link inside the application as a link', links, async () => {
			await symlink('app.asar', join(f.layout.resources, 'relative-link'));
			assert.ok((await packageInventory(f.application)).some(file => file.type === 'link' && file.target === 'app.asar'));
		});
		test('the inventory rejects a link that escapes the application', links, async () => {
			await symlink('/etc/passwd', join(f.layout.resources, 'external-link'));
			await assert.rejects(packageInventory(f.application), /escapes/);
		});
		test('the inventory records an executable permission change', links, async () => {
			const before = await packageInventory(f.application);
			await chmod(f.layout.native, 0o755);
			assert.notDeepStrictEqual(await packageInventory(f.application), before);
		});
		test('a failing builder leaves the previous installers in place', async () => {
			await mkdir(f.output, { recursive: true });
			await writeFile(join(f.output, 'previous'), 'completed output');
			await assert.rejects(buildInstallers(f.root, never(), failingBuilder), /installer tool failed/);
			assert.strictEqual(await readFile(join(f.output, 'previous'), 'utf8'), 'completed output');
		});
		test('an unsigned application\'s installers are not notarized', async () => {
			let notarized = 0;
			await buildInstallers(f.root, never(), {
				...builder, notarize: () => {
					notarized++;
					return Promise.resolve();
				}
			});
			assert.strictEqual(notarized, 0);
		});
		if (isMacintosh) {
			const signing = { VNILINK_MACOS_SIGNING_IDENTITY: 'Developer ID Application: Test', VNILINK_MACOS_NOTARY_PROFILE: 'test-profile' };
			test('a signed application\'s disk image is notarized before its checksum is taken', async () => {
				const signed = await checkout('developer-id');
				try {
					const images: string[] = [];
					await buildInstallers(signed.root, never(), {
						...builder,
						environment: signing,
						notarize: async (_processes, image, identity) => {
							images.push(`${basename(image)} ${identity.notaryProfile}`);
							await writeFile(image, 'stapled installer artifact');
						},
					});
					assert.deepStrictEqual(images, ['VniLink-test.dmg test-profile']);
					const manifest = await verifyInstallerFiles(signed.output, product);
					assert.strictEqual(manifest.artifacts[0].sha256, await fileSha256(join(signed.output, 'VniLink-test.dmg')));
				} finally {
					await rm(signed.root, { recursive: true, force: true });
				}
			});
			test('a signed application without the notary profile is refused', async () => {
				const signed = await checkout('developer-id');
				try {
					await assert.rejects(buildInstallers(signed.root, never(), { ...builder, environment: {} }), /not set to notarize/);
				} finally {
					await rm(signed.root, { recursive: true, force: true });
				}
			});
		}
		if (isWindows) {
			test('a signed application\'s installer is signed with the same certificate, given only to the packager', async () => {
				const signed = await checkout('authenticode');
				try {
					let given: NodeJS.ProcessEnv = {};
					await buildInstallers(signed.root, never(), {
						environment: { VNILINK_WINDOWS_CERTIFICATE_FILE: 'release.pfx', VNILINK_WINDOWS_CERTIFICATE_PASSWORD: 'secret' },
						runBuilder: (_processes, args, _cwd, signing) => {
							given = signing;
							return writeArtifacts(args, 'VniLink-test');
						},
					});
					assert.deepStrictEqual(given, { WIN_CSC_LINK: 'release.pfx', WIN_CSC_KEY_PASSWORD: 'secret' });
				} finally {
					await rm(signed.root, { recursive: true, force: true });
				}
			});
			test('a signed application without its certificate is refused', async () => {
				const signed = await checkout('authenticode');
				try {
					await assert.rejects(buildInstallers(signed.root, never(), { ...builder, environment: {} }), /not set to sign its installer/);
				} finally {
					await rm(signed.root, { recursive: true, force: true });
				}
			});
		}
		test('a failing builder leaves no work directory behind', async () => {
			await assert.rejects(buildInstallers(f.root, never(), failingBuilder), /installer tool failed/);
			assert.ok(!(await readdir(join(f.root, 'dist/installers'))).some(name => name.startsWith('.installer-')));
		});
		test('a failing builder releases the assembly lock', async () => {
			await assert.rejects(buildInstallers(f.root, never(), failingBuilder), /installer tool failed/);
			await assert.rejects(readFile(`${f.application}.lock`), /ENOENT/);
		});
		test('the builder packages the prepackaged application and publishes nothing itself', async () => {
			let given: readonly string[] = [];
			await buildInstallers(f.root, never(), {
				runBuilder: (_processes, args) => {
					given = args;
					return writeArtifacts(args, 'VniLink-test');
				},
			});
			assert.ok(given.includes('--prepackaged'));
			assert.strictEqual(given.at(-1), 'never');
		});
		test('published installers verify against their checksums', async () => {
			await buildInstallers(f.root, never(), builder);
			await verifyInstallerFiles(f.output, product);
		});
		test('installer verification rejects a corrupted artifact', async () => {
			await buildInstallers(f.root, never(), builder);
			const manifest = await verifyInstallerFiles(f.output, product);
			await writeFile(join(f.output, manifest.artifacts[0].path), 'corrupt artifact');
			await assert.rejects(verifyInstallerFiles(f.output, product));
		});
		test('the builder is given a copy, not the verified application', async () => {
			let prepackaged = '';
			await buildInstallers(f.root, never(), {
				runBuilder: (_processes, args) => {
					prepackaged = argument(args, '--prepackaged');
					return writeArtifacts(args, 'VniLink-test');
				},
			});
			assert.ok(!prepackaged.startsWith(f.application));
		});
		test('what the builder adds to its copy leaves the verified application unchanged', async () => {
			const before = await packageInventory(f.application);
			await buildInstallers(f.root, never(), {
				runBuilder: async (_processes, args) => {
					// As electron-builder's deb target adds the sandbox's AppArmor profile to the application it packages.
					await writeFile(join(argument(args, '--prepackaged'), 'added-by-builder'), 'builder resource');
					await writeArtifacts(args, 'VniLink-test');
				},
			});
			assert.deepStrictEqual(await packageInventory(f.application), before);
		});
		suite('while an assembly holds the lock', () => {
			beforeEach(async () => {
				// This process stands in for an assembly that is still running.
				await mkdir(`${f.application}.lock`);
				await writeFile(join(`${f.application}.lock`, 'owner.json'), JSON.stringify({ pid: process.pid }));
			});
			test('installer generation is refused', async () => {
				await assert.rejects(buildInstallers(f.root, never(), builder), /is publishing here/);
			});
			test('the builder is not started', async () => {
				let calls = 0;
				await assert.rejects(buildInstallers(f.root, never(), {
					runBuilder: () => {
						calls++;
						return Promise.resolve();
					},
				}));
				assert.strictEqual(calls, 0);
			});
		});
		suite('when cancelled after the builder ran', () => {
			let outcome: unknown;
			beforeEach(async () => {
				await mkdir(f.output, { recursive: true });
				await writeFile(join(f.output, 'previous'), 'completed');
				const controller = new AbortController();
				outcome = await buildInstallers(f.root, controller.signal, {
					runBuilder: async (_processes, args) => {
						await writeArtifacts(args, 'cancelled');
						controller.abort(new Error('Installer generation cancelled'));
					},
				}).catch((error: unknown) => error);
			});
			test('installer generation rejects with the cancellation', () => {
				assert.ok(outcome instanceof Error);
				assert.match(outcome.message, /cancelled/);
			});
			test('the previous installers stay published', async () => {
				assert.strictEqual(await readFile(join(f.output, 'previous'), 'utf8'), 'completed');
			});
			test('nothing but the published installers remains', async () => {
				assert.deepStrictEqual(await readdir(join(f.root, 'dist/installers')), [f.target]);
			});
		});
	});
});
