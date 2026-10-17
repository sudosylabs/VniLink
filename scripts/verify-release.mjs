import { fileURLToPath } from 'node:url';
import { ReleaseVerifier } from '../build/lib/releaseVerifier.mjs';

const directory = fileURLToPath(new URL(`../.build/release/${process.platform}-${process.arch}/`, import.meta.url));

await new ReleaseVerifier(directory).verifyNative();
