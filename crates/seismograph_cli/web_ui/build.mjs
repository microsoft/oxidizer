// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { readFile } from "node:fs/promises";
import { build } from "esbuild";

const assets = "../src/commands/monitor/web/assets";
const notices = await Promise.all([
  "react/LICENSE", "react-dom/LICENSE", "scheduler/LICENSE",
  "dockview-react/LICENCE.md", "dockview/LICENCE.md", "dockview-core/LICENCE.md",
].map(path =>
  readFile(`node_modules/${path}`, "utf8").then(license => license.trim())
));
const license = [...new Set(notices)].join("\n\n");
await build({
  entryPoints: ["main.jsx"],
  bundle: true,
  minify: true,
  jsx: "automatic",
  legalComments: "inline",
  banner: { js: `/* Third-party licenses:\n${license}\n*/`, css: `/* Dockview license:\n${notices[3]}\n*/` },
  outfile: `${assets}/monitor.js`,
});
