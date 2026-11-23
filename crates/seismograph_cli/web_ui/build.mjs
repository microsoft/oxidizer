// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { readFile } from "node:fs/promises";
import { build } from "esbuild";

const assets = "../src/commands/monitor/web/assets";
const notices = await Promise.all(["react", "react-dom", "scheduler"].map(name =>
  readFile(`node_modules/${name}/LICENSE`, "utf8").then(license => license.trim())
));
const license = [...new Set(notices)].join("\n\n");
await build({
  entryPoints: ["main.jsx"],
  bundle: true,
  minify: true,
  jsx: "automatic",
  legalComments: "inline",
  banner: { js: `/* React, React DOM, and Scheduler license:\n${license}\n*/` },
  outfile: `${assets}/monitor.js`,
});
