import { spawn } from 'node:child_process';
import { platform } from 'node:os';
import { build } from 'esbuild';
import { cp, mkdir, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { buildOptions, resources } from './tasks.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
// The VPX Windows libraries use GNU's C ABI and runtime. Keep every native
// dependency on that toolchain instead of mixing GNU and MSVC archives.
const nativeTarget = platform() === 'win32' ? 'x86_64-pc-windows-gnu' : undefined;

await new Promise((resolve, reject) => {
	const cargo = spawn('cargo', ['build', '--locked', '--target-dir', 'target/electron', ...(nativeTarget ? ['--target', nativeTarget] : []), '-p', 'vnilink-node'], { cwd: root, stdio: 'inherit' });
	cargo.once('error', reject);
	cargo.once('exit', code => {
		if (code === 0) {
			resolve();
		} else {
			reject(new Error('Native bridge build failed'));
		}
	});
});

await rm(`${root}out`, { recursive: true, force: true });

await mkdir(`${root}out/renderer`, { recursive: true });

await mkdir(`${root}out/native`, { recursive: true });

await mkdir(`${root}out/licenses`, { recursive: true });

await Promise.all([
	...buildOptions(root).map(options => build(options)),
	...resources(root).map(({ source, output }) => cp(source, output, { recursive: true })),
	cp(`${root}target/electron/${nativeTarget ? `${nativeTarget}/` : ''}debug/${platform() === 'win32' ? 'vnilink_node.dll' : platform() === 'darwin' ? 'libvnilink_node.dylib' : 'libvnilink_node.so'}`, `${root}out/native/vnilink.node`),
]);
