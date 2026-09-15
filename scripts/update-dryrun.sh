#!/usr/bin/env bash
#
# 本地演练自动更新：不发版也能把「检查更新 → 后台下载 → 底部进度条 → 完成提示」走一遍。
#
# 为什么需要它：更新清单 latest.json 只在 CI 发 Release 时生成。所以在正式发版之前，
# 应用里点「检查更新」只会因为 GitHub 上没有 latest.json 而报「没有返回版本清单」，
# 看不到进度条，也就没法验证这套交互。本脚本在本地造一份同构的清单 + 已签名安装包，
# 用本机 HTTP 服务顶上更新源，让开发版真的认为「有新版本」。
#
# 用法：
#   bash scripts/update-dryrun.sh              # 版本号自动取当前版本 +1（补丁位）
#   bash scripts/update-dryrun.sh 0.2.0        # 也可指定（必须大于 src-tauri/Cargo.toml 里的版本）
#   PORT=9000 bash scripts/update-dryrun.sh    # 换端口（默认自动挑一个空闲端口）
#
# 脚本会一直前台跑着（Ctrl-C 结束）。另开一个终端按它打印的命令启动开发版即可。
#
# ⚠️ 演练用的安装包是**占位文件**（内容随便，只用来验签），所以请到「已下载完成、重启后生效」
#    为止，不要点「立即重启」：开发版不是 .app 包，安装时插件会把 current_exe 的父目录
#    （也就是 target/debug）当成安装目标，既无意义又可能弄脏构建产物。
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/target/update-dryrun"        # 放 target/ 下，天然不会进版本库
KEY="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/myredis-updater.key}"

die() { echo "✗ $*" >&2; exit 1; }

command -v node >/dev/null 2>&1 || die "需要 node"
command -v python3 >/dev/null 2>&1 || die "需要 python3 来起本地静态服务"

if [ -n "${PORT:-}" ]; then
  port="$PORT"
else
  # 自动挑空闲端口：8899 之类的固定端口经常被别的本地服务占着
  port="$(python3 -c 'import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()')"
fi

# ---------- 版本号：默认取当前版本 +1（补丁位），保证比应用自己的版本新 ----------
current_version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/src-tauri/Cargo.toml" | head -1)"
[ -n "$current_version" ] || die "读不到 src-tauri/Cargo.toml 里的 version"

if [ $# -ge 1 ]; then
  version="$1"
else
  version="$(node -e '
    const [major, minor, patch] = process.argv[1].split(".").map(Number);
    console.log(`${major}.${minor}.${patch + 1}`);
  ' "$current_version")"
fi

# 只有比当前版本大，插件才会认为「有新版本」
node -e '
  const toNum = (v) => v.split(/[.\-+]/).slice(0, 3).map(Number);
  const [dry, current] = process.argv.slice(1).map(toNum);
  const newer = dry[0] > current[0]
    || (dry[0] === current[0] && (dry[1] > current[1] || (dry[1] === current[1] && dry[2] > current[2])));
  if (!newer) {
    console.error(`✗ 演练版本 ${process.argv[1]} 不比当前版本 ${process.argv[2]} 大，插件不会认为是新版本`);
    process.exit(1);
  }
' "$version" "$current_version"

[ -f "$KEY" ] || die "找不到签名私钥 ${KEY}（可用 TAURI_SIGNING_PRIVATE_KEY_PATH 指定）。公钥/私钥说明见 DEVELOPMENT.md §8.9"

# 私钥可能带密码（scripts/rotate-signing-key.sh 轮换时用自定义密码生成）：
# 给了环境变量就用它；没给且在终端里就问一次（直接回车 = 无密码）
if [ -n "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+set}" ]; then
  KEY_PASSWORD="$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
elif [ -t 0 ]; then
  read -rsp "签名私钥密码（无密码直接回车）: " KEY_PASSWORD
  echo
else
  KEY_PASSWORD=""
fi

# ---------- 造一份「已签名的安装包 + 清单」 ----------
rm -rf "$OUT"
mkdir -p "$OUT"

artifact_name="myredis-v${version}-macos-universal.app.tar.gz"
artifact="$OUT/$artifact_name"
printf '麦地缓存 %s 本地演练占位包（内容不重要，只用于验证签名与下载流程）\n' "$version" > "$artifact"

echo "• 签名 ${artifact_name}"
# 与 CI 完全相同的环境变量形式，顺带验证私钥可用
(cd "$ROOT/src-tauri" && TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")" \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$KEY_PASSWORD" \
  cargo tauri signer sign "$artifact" >/dev/null) || die "签名失败（私钥或密码不对？）"
[ -s "$artifact.sig" ] || die "签名文件是空的"

echo "• 生成 latest.json（version=${version}）"
node -e '
  const fs = require("node:fs");
  const [sigPath, outPath, version, port, name] = process.argv.slice(1);
  const manifest = {
    version,
    notes: `本地演练版本 ${version}（不是真的发布包）`,
    pub_date: new Date().toISOString(),
    platforms: {},
  };
  // 键名必须与运行端算出的 target 一致；macOS 出 universal 单包，两个键指向同一个文件
  for (const key of ["darwin-aarch64", "darwin-x86_64"]) {
    manifest.platforms[key] = {
      signature: fs.readFileSync(sigPath, "utf8").trim(),
      url: `http://127.0.0.1:${port}/${name}`,
    };
  }
  fs.writeFileSync(outPath, `${JSON.stringify(manifest, null, 2)}\n`);
' "$artifact.sig" "$OUT/latest.json" "$version" "$port" "$artifact_name"

# ---------- 自检：签名必须能被 tauri.conf.json 里那把公钥验过 ----------
# 这一步和客户端下载后的校验等价，能提前暴露「用了别的私钥去签」这类问题
if command -v minisign >/dev/null 2>&1; then
  node -e '
    const fs = require("node:fs");
    const [configPath, pubOut, sigIn, sigOut] = process.argv.slice(1);
    const pubkey = JSON.parse(fs.readFileSync(configPath, "utf8")).plugins.updater.pubkey;
    // 配置里存的、以及 .sig 文件里的都是 base64，minisign 要的是解码后的原文
    fs.writeFileSync(pubOut, Buffer.from(pubkey, "base64"));
    fs.writeFileSync(sigOut, Buffer.from(fs.readFileSync(sigIn, "utf8").trim(), "base64"));
  ' "$ROOT/src-tauri/tauri.conf.json" "$OUT/pubkey.pub" "$artifact.sig" "$OUT/sig.minisig"
  minisign -V -p "$OUT/pubkey.pub" -x "$OUT/sig.minisig" -m "$artifact" >/dev/null \
    || die "签名验不过配置里的公钥，客户端也会拒掉这个包"
  echo "• 签名校验通过（与 tauri.conf.json 的公钥配对）"
else
  echo "• 跳过 minisign 自检（未安装 minisign），不影响演练"
fi

# ---------- 起本地静态服务 ----------
python3 -m http.server "$port" --directory "$OUT" >"$OUT/server.log" 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT

sleep 1
curl -fsS "http://127.0.0.1:${port}/latest.json" >/dev/null || die "本地服务没起来，看 ${OUT}/server.log"
curl -fsS "http://127.0.0.1:${port}/${artifact_name}" >/dev/null || die "安装包下载地址不通"

cat <<EOF

✓ 演练更新源已就绪：http://127.0.0.1:${port}（清单与安装包都在 ${OUT}）

另开一个终端，让开发版把更新源指向本地，然后重启应用：

  cd ${ROOT}/src-tauri
  cargo tauri dev --config '{"plugins":{"updater":{"endpoints":["http://127.0.0.1:${port}/latest.json"]}}}'

（--config 只覆盖 endpoints，公钥等其它配置照旧；去掉这个参数就是正常的线上更新源。）

应用起来后点标题栏的「检查更新」，应当依次看到：
  1. 弹窗「发现新版本 v${version}」；
  2. 点「立即更新」后底部出现进度条（这个包很小，可能一闪而过）；
  3. 下载完成弹「更新已就绪」，底部的进度条变成绿色并带「立即重启」。

⚠️ 到这里就好，不要点「立即重启」——占位包解不出 .app，安装只会失败/弄脏 target/debug。
   要验证真正的安装与重启，得发一个带 latest.json 的 Release（见 DEVELOPMENT.md §8.9）。

Ctrl-C 结束本地更新源。
EOF

wait "$server_pid"
