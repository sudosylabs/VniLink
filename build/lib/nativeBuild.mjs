/** Build identity couples source inputs, target/profile and the staged napi binary. */
import { createHash } from 'node:crypto';
import { cp, readdir, readFile, writeFile } from 'node:fs/promises';
import { join, relative } from 'node:path';

export class NativeBuild {
	constructor(root, processes, target = process.env.CARGO_BUILD_TARGET ?? (process.platform === 'win32' ? 'x86_64-pc-windows-gnu' : undefined), profile = 'dev', environment = {}) {
		if (!['dev', 'release'].includes(profile)) {
			throw new Error('Unsupported native build profile');
		}
		this.root = root;
		this.processes = processes;
		this.target = target;
		this.profile = profile;
		this.environment = environment;
	}

	async identity() {
		const hash = createHash('sha256');
		const files = [];
		const collect = async directory => {
			for (const entry of await readdir(directory, { withFileTypes: true })) {
				if (['target', '.git'].includes(entry.name)) {
					continue;
				}
				const path = join(directory, entry.name);
				if (entry.isDirectory()) {
					await collect(path);
				} else if (entry.isFile() && !path.endsWith('AGENTS.md') && !path.endsWith('.md')) {
					files.push(path);
				}
			}
		};
		for (const directory of ['crates', 'vendor', '.cargo']) {
			await collect(join(this.root, directory));
		}
		files.push(...['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'].map(name => join(this.root, name)));
		for (const path of files.sort()) {
			hash.update(relative(this.root, path));
			hash.update('\0');
			hash.update(await readFile(path));
			hash.update('\0');
		}
		return { source: hash.digest('hex'), target: this.target ?? null, profile: this.profile, platform: process.platform, architecture: process.arch, ...(Object.keys(this.environment).length ? { environment: this.environment } : {}) };
	}

	async build() {
		if (this.target) {
			const architecture = this.target.startsWith('aarch64-') ? 'arm64' : this.target.startsWith('x86_64-') ? 'x64' : undefined;
			const platform = this.target.includes('apple-darwin') ? 'darwin' : this.target.includes('windows') ? 'win32' : this.target.includes('linux') ? 'linux' : undefined;
			if (architecture !== process.arch || platform !== process.platform) {
				throw new Error('The staged Electron addon must target the current host platform and architecture');
			}
		}
		this.before = await this.identity();
		await this.processes.run('cargo', ['build', '--locked', '--target-dir', 'target/electron', ...(this.target ? ['--target', this.target] : []), ...(this.profile === 'release' ? ['--release'] : []), '-p', 'vnilink-node'], { cwd: this.root, env: { ...process.env, ...this.environment } });
	}

	async stage(outputDirectory = join(this.root, 'out')) {
		const identity = await this.identity();
		if (JSON.stringify(identity) !== JSON.stringify(this.before)) {
			throw new Error('Native sources changed during build; rebuild before launching');
		}
		const filename = process.platform === 'win32' ? 'vnilink_node.dll' : process.platform === 'darwin' ? 'libvnilink_node.dylib' : 'libvnilink_node.so';
		const source = join(this.root, 'target/electron', this.target ?? '', this.profile === 'release' ? 'release' : 'debug', filename);
		const output = join(outputDirectory, 'native/vnilink.node');
		await cp(source, output);
		const binary = createHash('sha256').update(await readFile(output)).digest('hex');
		await writeFile(join(outputDirectory, 'native/build.json'), JSON.stringify({ ...identity, binary }) + '\n');
	}

	async assertFresh(outputDirectory = join(this.root, 'out')) {
		try {
			const saved = JSON.parse(await readFile(join(outputDirectory, 'native/build.json'), 'utf8'));
			const identity = await this.identity();
			const binary = createHash('sha256').update(await readFile(join(outputDirectory, 'native/vnilink.node'))).digest('hex');
			if (JSON.stringify(saved) !== JSON.stringify({ ...identity, binary })) {
				throw new Error('Native build identity differs');
			}
		} catch (error) {
			throw new Error('Native addon missing or stale. Run npm run build. Rust is not watched.', { cause: error });
		}
	}
}
