// Ra bản mới: node tools/release.mjs 1.0.1 "Ghi chú ngắn"
// Nâng số phiên bản ở 3 nơi, commit, gắn tag vX.Y.Z và đẩy lên GitHub. GitHub Actions sẽ build bộ cài và đăng bản cập nhật.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

const [version, note] = process.argv.slice(2);
if (!/^\d+\.\d+\.\d+$/.test(version || "")) {
  console.error('Cách dùng: node tools/release.mjs 1.0.1 "Ghi chú"');
  process.exit(1);
}

const git = (...args) => execFileSync("git", args, { stdio: "inherit" });
const out = execFileSync("git", ["status", "--porcelain"], { encoding: "utf8" }).trim();
if (out) {
  console.error("Còn thay đổi chưa commit. Hãy commit trước rồi mới ra bản mới:\n" + out);
  process.exit(1);
}

const bump = (file, pattern, replacement) => {
  const text = readFileSync(file, "utf8");
  if (!pattern.test(text)) throw new Error(`Không tìm thấy số phiên bản trong ${file}`);
  writeFileSync(file, text.replace(pattern, replacement));
};
bump("package.json", /("version":\s*")[^"]+(")/, `$1${version}$2`);
bump("src-tauri/tauri.conf.json", /("version":\s*")[^"]+(")/, `$1${version}$2`);
bump("src-tauri/Cargo.toml", /^(version\s*=\s*")[^"]+(")/m, `$1${version}$2`);

git("add", "package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml");
git("commit", "-m", `Phiên bản ${version}${note ? `: ${note}` : ""}`);
git("tag", "-a", `v${version}`, "-m", note || `Phiên bản ${version}`);
git("push", "origin", "HEAD", `v${version}`);
console.log(`\nĐã đẩy v${version}. Theo dõi tiến trình build tại tab Actions của repo.`);
