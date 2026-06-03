import { readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const offckbRoot = dirname(require.resolve("@offckb/cli/package.json"));

const specPath = join(offckbRoot, "ckb/devnet/specs/dev.toml");
const ckbTomlPath = join(offckbRoot, "ckb/devnet/ckb.toml");

let spec = await readFile(specPath, "utf8");
spec = spec.replace(
  /^epoch_duration_target = .+$/m,
  "epoch_duration_target = 1",
);
spec = spec.replace(/^genesis_epoch_length = .+$/m, "genesis_epoch_length = 1");
await writeFile(specPath, spec);

let ckbToml = await readFile(ckbTomlPath, "utf8");
ckbToml = ckbToml.replace(
  /^modules = \[(.+)\]$/m,
  (line) => (line.includes("IntegrationTest") ? line : line.replace("]", ', "IntegrationTest"]')),
);
await writeFile(ckbTomlPath, ckbToml);
