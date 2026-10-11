import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The workspace version, read from ../Cargo.toml at build time so the site
// never says an older number than the binaries. Vercel builds from the
// repository root with Root Directory `site`, so the file is always there.
const cargoToml = readFileSync(fileURLToPath(new URL("../../../Cargo.toml", import.meta.url)), "utf8");
const m = cargoToml.match(/^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/ms);
if (!m) throw new Error("site: no workspace version in Cargo.toml");

/** The current release, e.g. `0.2.0`, from the workspace `Cargo.toml`. */
export const VERSION: string = m[1];
