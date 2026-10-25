// dartq skills extension for the pi coding agent.
//
// Registers this package's shared skills directory (the dartq SKILL.md set)
// as a skill path. Deliberately dependency-free: no type imports, so the
// same file runs under pi and its oh-my-pi (omp) fork without installing
// the agent types package.

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const extensionDir = dirname(fileURLToPath(import.meta.url));
// <package>/adapters/pi/extensions -> <package>/skills (the SKILL.md set)
const skillsDir = resolve(extensionDir, "../../../skills");

export default function dartqPiExtension(pi: any) {
	pi.on("resources_discover", async () => ({
		skillPaths: [skillsDir],
	}));
}
