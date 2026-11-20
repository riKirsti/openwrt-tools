import { readFile } from "node:fs/promises";

const manifest = JSON.parse(await readFile(new URL("./package.json", import.meta.url), "utf8"));
const pinned = manifest.dependencies["@opencode/cli"];
const model = manifest.config.model.split("/")[1];

async function get(url) {
  const result = await fetch(url, { signal: AbortSignal.timeout(5000) });
  if (!result.ok) throw new Error("Upstream metadata is unavailable");
  return result.json();
}

const registry = "https://registry.npmjs.org/@opencode%2fcli";
const checks = await Promise.allSettled([
  get(`${registry}/${pinned}`),
  get(`${registry}/latest`),
  get("https://opencode.ai/inference/openai/v1/models"),
]);

if (checks[0].status === "fulfilled" && checks[0].value.deprecated) {
  console.log("::warning::The pinned OpenCode package is deprecated. Check the official install and migration guides.");
}
if (checks[1].status === "fulfilled" && checks[1].value.version !== pinned) {
  console.log("::notice::OpenCode has a newer release. Review the official install and migration guides before updating the pin.");
}
if (checks[2].status === "fulfilled") {
  const entry = checks[2].value.data?.find(item => item.id === model);
  if (!entry || entry.deprecated || entry.status === "deprecated") {
    console.log("::warning::The pinned model is missing or deprecated in the official OpenCode catalog.");
  }
}
if (checks.some(check => check.status === "rejected")) {
  console.log("::notice::An OpenCode metadata check was unavailable. The locked CLI and runtime model verification still apply.");
}
