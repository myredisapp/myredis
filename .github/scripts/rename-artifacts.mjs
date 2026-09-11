// 把 tauri 产出的安装包改名成 myredis-<tag>-<platform>.<ext>，让 Release 上的下载文件名统一。
// 只在 CI 里执行（改的是构建产物，不回写仓库）。
// 用法：node rename-artifacts.mjs <bundleDir> <tag> <platform> <bundles>
//   例：node rename-artifacts.mjs src-tauri/target/universal-apple-darwin/release/bundle v0.2.0 macos-universal app,dmg
//
// tauri 的包名来自 productName（macOS / Windows 是中文「麦地缓存」，Linux 是 tauri.linux.conf.json
// 覆盖的 maidi-cache），直接发布在下载页上既不好认、也不统一，所以在打包后、上传前改一次名。
import { existsSync, readdirSync, renameSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';

// tauri 的 bundle kind -> 实际产出的安装包扩展名。
// 扩展名的大小写就是文件系统上的写法（.AppImage 不能写成 .appimage），改名时必须原样保留。
const BUNDLE_EXT = {
  dmg: '.dmg',
  deb: '.deb',
  appimage: '.AppImage',
  msi: '.msi',
  nsis: '.exe',
};
// app 产出的是 .app 目录、不是安装包，没有对应的文件需要收
const BUNDLE_DIR_ONLY = new Set(['app']);

const [bundleDir, tag, platform, bundles] = process.argv.slice(2);

if (!bundleDir || !tag || !platform || !bundles) {
  console.error('用法：node rename-artifacts.mjs <bundleDir> <tag> <platform> <bundles>');
  process.exit(1);
}

// tag 会直接进文件名，挡掉路径分隔符之类的字符
if (!/^[0-9A-Za-z.+-]+$/.test(tag)) {
  console.error(`tag "${tag}" 含非法字符，无法用作文件名`);
  process.exit(1);
}

if (!existsSync(bundleDir)) {
  console.error(`找不到打包目录 ${bundleDir}`);
  process.exit(1);
}

// 只处理本 job 声明要打的 kind：同一个 job 的 bundle 目录里若混进别的平台的产物
// （比如上次构建的残留），宁可不认，也不能给它贴上本平台的名字
const wanted = new Set();
for (const kind of bundles.split(',').map((k) => k.trim().toLowerCase())) {
  if (!kind || BUNDLE_DIR_ONLY.has(kind)) continue;
  const ext = BUNDLE_EXT[kind];
  if (!ext) {
    console.error(`不认识的 bundle 类型 "${kind}"，请在 rename-artifacts.mjs 的 BUNDLE_EXT 里补上`);
    process.exit(1);
  }
  wanted.add(ext);
}

// 只看 bundle/<一层子目录>/<文件>，不递归：更深层的都是 tauri 的临时产物
const found = new Map();
for (const entry of readdirSync(bundleDir)) {
  const sub = join(bundleDir, entry);
  if (!statSync(sub).isDirectory()) continue;

  for (const file of readdirSync(sub)) {
    const src = join(sub, file);
    if (!statSync(src).isFile()) continue;

    const ext = [...wanted].find((e) => file.endsWith(e));
    if (!ext) continue;

    // 每个扩展名只应有一个文件；出现第二个说明打包配置变了，宁可失败也不要悄悄覆盖
    if (found.has(ext)) {
      console.error(`${ext} 出现多个文件，无法确定该保留哪个：\n  ${found.get(ext)}\n  ${src}`);
      process.exit(1);
    }
    found.set(ext, src);
  }
}

// 声明的每个 kind 都要有产物，否则会发出一个少包的 Release
const missing = [...wanted].filter((ext) => !found.has(ext));
if (missing.length > 0) {
  console.error(`${bundleDir} 下缺少 ${missing.join(' / ')} 安装包（--bundles ${bundles}）`);
  process.exit(1);
}

for (const [ext, src] of found) {
  const dest = join(dirname(src), `myredis-${tag}-${platform}${ext}`);
  renameSync(src, dest);
  console.log(`${src} -> ${dest}`);
}
