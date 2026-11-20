// Cross-check a dst WM1/JSON export against dsh-whale-musume's whale-moe-core.js.
// The reference checkout is not part of this repo. If it is missing, the script
// skips (exit 0) so CI does not need the other repository.
//
//   node tools/sim/check_pet_migration.mjs \
//     /tmp/dsh-whale-musume/assets/whale-moe-core.js \
//     target/pet-wm1-fixture.txt \
//     target/pet-json-fixture.json

import fs from "fs";
import vm from "vm";

const [corePath, codePath, jsonPath] = process.argv.slice(2);
if (!corePath || !fs.existsSync(corePath)) {
  console.log("skip: reference whale-moe-core.js not found");
  process.exit(0);
}
if (!codePath || !jsonPath || !fs.existsSync(codePath) || !fs.existsSync(jsonPath)) {
  console.log("skip: fixtures missing; run cargo test wm1_roundtrip first");
  process.exit(0);
}

const sandbox = { module: { exports: {} }, exports: {} };
sandbox.module.exports = sandbox.exports;
vm.runInNewContext(fs.readFileSync(corePath, "utf8"), sandbox, { filename: corePath });
const core = sandbox.module.exports;
const now = 1763600000000;
const code = fs.readFileSync(codePath, "utf8").trim();
const json = fs.readFileSync(jsonPath, "utf8");
const fromCode = core.portableFromCode(code, now);
const fromJson = core.portableParse(json, now);
if (!fromCode.ok) {
  console.error("portableFromCode:", fromCode.error);
  process.exit(1);
}
if (!fromJson.ok) {
  console.error("portableParse:", fromJson.error);
  process.exit(1);
}
const d = fromCode.payload.data;
if (d.mood !== "64" || d.title !== "主人" || d.achievements !== "first-pat") {
  console.error("unexpected code payload", d);
  process.exit(1);
}
if (!String(fromJson.payload.data.quests || "").includes("signin-1")) {
  console.error("quests dropped", fromJson.payload.data.quests);
  process.exit(1);
}
const rebuilt = core.portableCode(core.portableBuild({
  mood: "64", affinity: "42", satiety: "80", lastSignin: "", signinStreak: "0",
  achievements: "first-pat,nope", companionSince: "", level: "1",
  badge: "", title: "主人", selfName: "大肥鱼",
}, { appVersion: "dst 0.2.0" }, now));
if (rebuilt !== code) {
  console.error("WM1 is not byte-identical to portableCode");
  process.exit(1);
}
console.log("ok: reference portableFromCode / portableParse / portableCode match");
