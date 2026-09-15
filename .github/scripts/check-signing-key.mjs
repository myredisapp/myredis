// 校验 CI 里的更新签名私钥：能不能用给的密码解开，以及是不是与 tauri.conf.json 里的
// 公钥配对。发布流水线的第一步就跑它（见 .github/workflows/release.yml 的 check-deploy-env）。
//
// 为什么必须前置：私钥或密码配错时 `tauri build` 依然会成功 —— 只是产出的 .sig 客户端
// 验不过，或者干脆没有 .sig。症状是「已安装的应用永远收不到更新」这种静默失败，
// 等几十分钟构建跑完、Release 也发出去了才发现，而已经发出去的版本收不回。
//
// 判定手段是真签一次：tauri 的私钥以加密形式存放，密码不对时解密就会失败；签名结果里
// 带着 key id，和公钥里的 key id 比对即可确认是不是同一对密钥（不用引入验签依赖）。
//
// 用法：node check-signing-key.mjs [tauri.conf.json 路径]
// 环境变量（与 tauri CLI 同名，直接透传给签名命令）：
//   TAURI_SIGNING_PRIVATE_KEY           私钥内容（base64），CI 用这个
//   TAURI_SIGNING_PRIVATE_KEY_PATH      私钥文件路径，本地用它更方便
//   TAURI_SIGNING_PRIVATE_KEY_PASSWORD  私钥密码；「已设置但为空」= 无密码（tauri 语义）
//   TAURI_SIGNER_CMD                     可选，指定签名命令，如 "cargo tauri"
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const fail = (...messages) => {
  for (const message of messages) {
    console.error(`::error::${message}`);
  }
  process.exit(1);
};

const configPath = process.argv[2] ?? 'src-tauri/tauri.conf.json';

const key = process.env.TAURI_SIGNING_PRIVATE_KEY ?? '';
const keyPath = process.env.TAURI_SIGNING_PRIVATE_KEY_PATH ?? '';
// 「未设置」在 CI 与「设置为空」都表现为空字符串，统一按无密码处理：
// 密码真错时下面的签名会失败。但必须显式传给子进程一个值 —— tauri 对**未设置**的
// 变量会尝试交互式索要密码，在 CI 里会直接卡住。
const password = process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? '';

if (!key && !keyPath) {
  fail(
    '缺少 TAURI_SIGNING_PRIVATE_KEY（或 TAURI_SIGNING_PRIVATE_KEY_PATH）：更新包签名私钥。',
    '该私钥丢失后，已发布的应用将永远收不到自动更新，请从备份恢复并妥善保管。',
    '生成密钥对：tauri signer generate -w ~/.tauri/myredis-updater.key',
  );
}
if (!key && keyPath && !existsSync(keyPath)) {
  fail(`TAURI_SIGNING_PRIVATE_KEY_PATH 指向的文件不存在: ${keyPath}`);
}
if (key && keyPath) {
  // 两个都设时 tauri CLI 会以「参数冲突」直接失败，与其让签名步骤报 clap 的原文，不如在这里说清楚
  fail(
    '同时设置了 TAURI_SIGNING_PRIVATE_KEY 与 TAURI_SIGNING_PRIVATE_KEY_PATH，两者互斥。',
    '请只保留一个：CI 用 TAURI_SIGNING_PRIVATE_KEY，本地用 TAURI_SIGNING_PRIVATE_KEY_PATH 更方便。',
  );
}

// ---------------------------------------------------------------------------
// 公钥与清单配置：都在 tauri.conf.json 里，签名私钥必须与这把公钥配对
// ---------------------------------------------------------------------------
if (!existsSync(configPath)) {
  fail(`找不到 ${configPath}（可在命令行第一个参数指定路径）`);
}

let config;
try {
  config = JSON.parse(readFileSync(configPath, 'utf8'));
} catch (err) {
  fail(`${configPath} 不是合法 JSON: ${err.message}`);
}

const updater = config.plugins?.updater ?? {};
const pubkey = updater.pubkey ?? '';
if (!pubkey) {
  fail(`${configPath} 缺少 plugins.updater.pubkey：客户端没有公钥就无法校验更新包`);
}
if (!Array.isArray(updater.endpoints) || updater.endpoints.length === 0) {
  fail(`${configPath} 缺少 plugins.updater.endpoints：客户端不知道该去哪儿查更新`);
}
if (config.bundle?.createUpdaterArtifacts !== true) {
  fail(`${configPath} 的 bundle.createUpdaterArtifacts 不为 true：构建不会产出 .sig，更新包无法被验签`);
}

// 兼容两种存放形式：minisign 原文，或原文的 base64（tauri 的 .sig 文件与配置里的
// pubkey 都是后者 —— base64 里不会出现空格，所以用注释行里的空格就能分辨）
const minisignText = (raw) =>
  raw.includes('untrusted comment') ? raw : Buffer.from(raw.trim(), 'base64').toString('utf8');

// 配置里存的是 minisign 公钥文件的 base64（也容忍直接贴文本，便于排查）
const pubkeyText = minisignText(pubkey);

// minisign 的公钥与签名载荷结构都是：算法[2] | key id[8] | …；key id 是明文，
// 注释行里那串十六进制就是它按字节倒序的写法。
const keyIdOf = (payload) => {
  if (payload.length < 10) return null;
  return [...payload.subarray(2, 10)]
    .reverse()
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
    .toUpperCase();
};

/** 取 minisign 文本里第一条非注释行的载荷（公钥文件与签名文件都是这个位置）。 */
const payloadOf = (text) => {
  const line = text
    .split('\n')
    .map((each) => each.trim())
    .filter(Boolean)
    .find((each) => !each.startsWith('untrusted comment') && !each.startsWith('trusted comment'));
  return line ? Buffer.from(line, 'base64') : Buffer.alloc(0);
};

const expectedKeyId = keyIdOf(payloadOf(pubkeyText));
if (!expectedKeyId) {
  fail(`${configPath} 里的 plugins.updater.pubkey 解不出 key id，请确认它是 tauri signer generate 产出的公钥`);
}

// ---------------------------------------------------------------------------
// 真签一次：既验证密码，又拿到签名里的 key id
// ---------------------------------------------------------------------------
const resolveSigner = () => {
  if (process.env.TAURI_SIGNER_CMD) return process.env.TAURI_SIGNER_CMD.split(' ').filter(Boolean);
  // npm 装的 tauri（CI），或 cargo 装的 cargo-tauri（cargo 会把 `cargo tauri` 派发给它）
  for (const candidate of [['tauri'], ['cargo', 'tauri']]) {
    const [bin, ...args] = candidate;
    // Windows 上 npm 只装出 tauri.cmd / tauri.ps1 两个 shim，Node 不带 shell 直接
    // spawn 找不到可执行体（ENOENT）；走 cmd.exe 才能解析 .cmd
    const probe = spawnSync(bin, [...args, '--version'], {
      encoding: 'utf8',
      shell: process.platform === 'win32',
    });
    if (!probe.error && probe.status === 0) return candidate;
  }
  return null;
};

const signer = resolveSigner();
if (!signer) {
  fail('找不到 tauri CLI。CI 上应由 `npm install -g @tauri-apps/cli` 提供，本地可设 TAURI_SIGNER_CMD=cargo tauri');
}

const workDir = mkdtempSync(join(tmpdir(), 'maidi-signing-key-'));
try {
  const probeFile = join(workDir, 'probe.bin');
  writeFileSync(probeFile, `maidi-cache signing key check ${new Date().toISOString()}\n`);

  // clap 把「变量存在但为空」当成「提供了该参数」：空的私有钥路径会报
  // “a value is required for '--private-key-path'”，两个都为空值则直接互斥报错。
  // 所以空值一律从子进程环境里摘掉，与 tauri build 的实际处境对齐（CI 里这些变量干脆不存在）。
  const childEnv = { ...process.env };
  for (const name of ['TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH']) {
    if (!childEnv[name]) delete childEnv[name];
  }
  // 密码必须显式给值：tauri 对**未设置**的密码会尝试交互式索要，会把 CI 卡住
  childEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = password;

  const sign = spawnSync(signer[0], [...signer.slice(1), 'signer', 'sign', probeFile], {
    encoding: 'utf8',
    env: childEnv,
    shell: process.platform === 'win32',
  });

  // CLI 会把同一条错误同时打到 stdout 与 stderr，去重后再转成日志，免得刷两遍
  const signerOutput = [
    ...new Set(
      `${sign.stdout ?? ''}${sign.stderr ?? ''}`
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean),
    ),
  ].join('\n');
  if (sign.error || sign.status !== 0) {
    fail(
      `用当前私钥/密码签名失败（退出码 ${sign.status}）。`,
      ...(sign.error ? [`  ${sign.error.message}`] : []),
      ...signerOutput.split('\n').map((line) => `  签名命令输出: ${line}`),
      '常见原因：TAURI_SIGNING_PRIVATE_KEY 内容不完整（应为 tauri signer generate 产出的整段 base64）；',
      '或 TAURI_SIGNING_PRIVATE_KEY_PASSWORD 与私钥的实际密码不一致（轮换密钥后最容易踩：密码换了，secret 没换）。',
    );
  }

  const sigFile = `${probeFile}.sig`;
  if (!existsSync(sigFile)) {
    fail('签名命令成功但没产出 .sig 文件，请检查 tauri CLI 版本');
  }

  const actualKeyId = keyIdOf(payloadOf(minisignText(readFileSync(sigFile, 'utf8'))));
  if (!actualKeyId) {
    fail('签名结果解不出 key id，请检查 tauri CLI 版本');
  }

  if (actualKeyId !== expectedKeyId) {
    fail(
      `私钥与公钥不配对：签名私钥 key id = ${actualKeyId}，${configPath} 里的公钥 key id = ${expectedKeyId}。`,
      '用这对密钥签出来的更新包，客户端一律验签失败（表现为下载完成却装不上），请换回配套的私钥/公钥。',
      `公钥应取自与私钥同一次 tauri signer generate 的 .pub 文件，配置位置：${configPath} → plugins.updater.pubkey`,
    );
  }

  console.log(`==> 签名私钥可用，且与公钥配对（key id ${actualKeyId}）`);
  console.log(`    私钥来源：${key ? 'TAURI_SIGNING_PRIVATE_KEY' : keyPath}`);
  console.log(`    密码：${password ? '已设置' : '未设置（按无密码处理）'}`);
  console.log(`    更新源：${updater.endpoints.join(', ')}`);
} finally {
  rmSync(workDir, { recursive: true, force: true });
}
