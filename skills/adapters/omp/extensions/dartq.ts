// dartq skills extension for oh-my-pi (omp) / pi.
//
// Registers this package's shared skills directory (the dartq SKILL.md set)
// as a skill path. The skills live outside this adapter directory, so the
// extension resolves them explicitly instead of relying on the conventional
// `skills/<name>/SKILL.md` layout at the package root. Deliberately
// dependency-free so it runs under both runtimes.

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const extensionDir = dirname(fileURLToPath(import.meta.url));
// <package>/adapters/omp/extensions -> <package>/skills (the SKILL.md set)
const skillsDir = resolve(extensionDir, "../../../skills");

export default function dartqOmpExtension(omp: any) {
	omp.on("resources_discover", async () => ({
		skillPaths: [skillsDir],
	}));
}
