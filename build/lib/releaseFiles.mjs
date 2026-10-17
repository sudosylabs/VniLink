/** Only source inputs belong in relinking material; generated files are never build inputs. */
import { basename } from 'node:path';

const generatedDirectories = new Set(['.git', '.build', 'target', 'node_modules', '__pycache__']);

export function releaseSourceFilter(source) {
	const name = basename(source);
	return !generatedDirectories.has(name) && name !== 'AGENTS.md' && !/\.(o|obj|a|so|dylib|dll|node|exe|pyc)$/.test(name);
}
