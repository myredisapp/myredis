// 把 tag 号写进 src-tauri/Cargo.toml 与 tauri.conf.json，让安装包版本号跟随 tag。
// 仅在 CI 里执行（工作区改动不回写仓库），用法：node set-version.mjs v0.2.0
import { readFileSync, writeFileSync } from 'node:fs';

const raw = (process.argv[2] ?? '').trim();
const version = raw.replace(/^[vV]/, '');

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`tag "${raw}" 里没有合法版本号，期望形如 v0.2.0 / v0.2.0-beta.1`);
  process.exit(1);
}

// 只替换 [package] 段里的 version，避免碰 [dependencies] 里带 version 字段的行
const cargoPath = 'src-tauri/Cargo.toml';
const lines = readFileSync(cargoPath, 'utf8').split('\n');
const pkgStart = lines.findIndex((line) => line.trim() === '[package]');
if (pkgStart === -1) throw new Error(`${cargoPath} 里找不到 [package] 段`);

let pkgEnd = lines.findIndex((line, i) => i > pkgStart && line.startsWith('['));
if (pkgEnd === -1) pkgEnd = lines.length;

const versionLine = lines.findIndex(
  (line, i) => i > pkgStart && i < pkgEnd && /^version\s*=/.test(line),
);
if (versionLine === -1) throw new Error(`${cargoPath} 的 [package] 段里找不到 version`);

lines[versionLine] = `version = "${version}"`;
writeFileSync(cargoPath, lines.join('\n'));

const confPath = 'src-tauri/tauri.conf.json';
const conf = JSON.parse(readFileSync(confPath, 'utf8'));
conf.version = version;
writeFileSync(confPath, `${JSON.stringify(conf, null, 2)}\n`);

console.log(`应用版本号已设为 ${version}`);
