// 生成 Tauri 自动更新用的清单 latest.json，内容与本次 Release 里的安装包一一对应。
//
// 只在 CI 里执行（读的是改名后的产物名，命名规则见 rename-artifacts.mjs）。
// 客户端从 tauri.conf.json 的 plugins.updater.endpoints 拉这个文件，比对版本号后
// 下载对应平台的包，再用同一平台条目里的 signature（就是 .sig 文件的内容）验签。
//
// 用法：node gen-latest-json.mjs <tag> <distDir> [notesFile] [outFile]
//   例：node gen-latest-json.mjs v0.2.0 dist /tmp/notes.md latest.json
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

// 平台键是 tauri 的 `OS-ARCH` 形式，必须与运行端算出来的 target 完全一致。
// macOS 出的是 universal 单包，所以 aarch64 / x86_64 两个键指向同一个 .app.tar.gz。
const TARGETS = [
  {
    keys: ['darwin-aarch64', 'darwin-x86_64'],
    file: (tag) => `myredis-${tag}-macos-universal.app.tar.gz`,
  },
  {
    keys: ['linux-x86_64'],
    file: (tag) => `myredis-${tag}-linux-x64.AppImage`,
  },
  {
    // Windows 只挂 NSIS（.exe）：msi 与 nsis 是两套各自独立的卸载信息，
    // 用 msi 去更新 nsis 装的机器会留下两份安装，所以下载页上的 .msi 不参与自动更新
    keys: ['windows-x86_64'],
    file: (tag) => `myredis-${tag}-windows-x64.exe`,
  },
];

const [tag, distDir, notesFile, outFile = 'latest.json'] = process.argv.slice(2);

if (!tag || !distDir) {
  console.error('用法：node gen-latest-json.mjs <tag> <distDir> [notesFile] [outFile]');
  process.exit(1);
}

if (!/^[0-9A-Za-z.+-]+$/.test(tag)) {
  console.error(`tag "${tag}" 含非法字符，无法拼出下载地址`);
  process.exit(1);
}

// latest.json 的 version 允许带前导 v，但统一按不带 v 发布，避免客户端比对时歧义
const version = tag.replace(/^[vV]/, '');

// 下载地址指向本 tag 的 Release 附件；用环境变量拿仓库名，本地跑时回退到默认仓库
const repo = process.env.GITHUB_REPOSITORY || 'myredisapp/myredis';
const urlBase = `https://github.com/${repo}/releases/download/${tag}`;

const platforms = {};
for (const target of TARGETS) {
  const name = target.file(tag);
  const artifact = join(distDir, name);
  const sigPath = `${artifact}.sig`;

  if (!existsSync(artifact)) {
    console.error(`找不到更新包 ${name}（在 ${distDir} 下）。改名步骤没生效，还是构建没出这个包？`);
    process.exit(1);
  }
  if (!existsSync(sigPath)) {
    console.error(`找不到签名文件 ${name}.sig。构建时没配 TAURI_SIGNING_PRIVATE_KEY，更新会被客户端拒掉。`);
    process.exit(1);
  }

  // signature 字段要的是签名文本本身，不是路径、也不是地址
  const signature = readFileSync(sigPath, 'utf8').trim();
  if (!signature) {
    console.error(`签名文件 ${name}.sig 是空的`);
    process.exit(1);
  }

  const url = `${urlBase}/${name}`;
  for (const key of target.keys) {
    platforms[key] = { signature, url };
  }
  console.log(`${name} -> ${target.keys.join(' / ')}`);
}

const notes = notesFile && existsSync(notesFile) ? readFileSync(notesFile, 'utf8').trim() : '';
const manifest = {
  version,
  notes: notes || `麦地缓存 ${tag}`,
  // RFC 3339，客户端解析失败时只是不显示日期，不影响更新
  pub_date: new Date().toISOString(),
  platforms,
};

writeFileSync(outFile, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`已生成 ${outFile}（version=${version}，${Object.keys(platforms).length} 个平台键）`);
