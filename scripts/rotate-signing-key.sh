#!/usr/bin/env bash
#
# 轮换自动更新签名密钥：用你自己的密码生成新的一对 → 换 tauri.conf.json 里的公钥
# → 更新 GitHub secret → 用 check-signing-key.mjs 自检（不通过就说明还没换好）。
#
# 密码为什么不写在命令里：私钥密码是长期凭据，脚本让你当场输入两次（不回显），
# 这样它不会进命令历史、也不会进聊天记录或日志。唯一的例外是生成的那一瞬间 ——
# `tauri signer generate` 只支持用 -p 参数传密码，所以本机 ps 里会短暂可见。
# 想完全避开这点，就自己跑 `tauri signer generate -w ~/.tauri/myredis-updater.key`
# 按提示交互输入，然后回来跑本脚本的 `SKIP_GENERATE=1` 分支。
#
# ⛔ 轮换是**冻结**的：只有在「还没有任何客户端带着旧公钥发出去」之前才可以换。
# 客户端的公钥是编译进安装包的（tauri.conf.json → plugins.updater.pubkey），
# 旧公钥一旦随安装包发出去，换私钥就等于那些用户再也收不到自动更新（只能手动重装）。
# 本项目 **v0.0.13 起已有正式版带着公钥发布**（v0.0.12 及更早没有更新模块），
# 所以脚本在动手之前先过一道闸门：打印当前公钥的 key id 与「哪些版本已经带着它发出去了」，
# 交互路径要输入 ROTATE 确认，非交互路径必须显式放行（ALLOW_ROTATE=1）。
# 决策口径见 DEVELOPMENT.md §3「明确不支持」表与 §8.9 的恢复 / 轮换说明。
#
# 用法：
#   bash scripts/rotate-signing-key.sh
#     ALLOW_ROTATE=1             显式放行轮换（非交互路径必须给；交互路径可输入 ROTATE 代替）
#     UPDATER_KEY_PASSWORD=xxx   不交互，直接用这个密码（自动化用）
#     SKIP_GENERATE=1            只做「换公钥 + 换 secret + 自检」，不重新生成密钥
#     SKIP_GH=1                  不动 CI secret（本地演练用）
#     KEY_PATH=... CONFIG_PATH=...  换默认路径（默认 ~/.tauri/myredis-updater.key
#                                 与 src-tauri/tauri.conf.json）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KEY="${KEY_PATH:-$HOME/.tauri/myredis-updater.key}"
CONFIG="${CONFIG_PATH:-$ROOT/src-tauri/tauri.conf.json}"

die() { echo "✗ $*" >&2; exit 1; }

[ -f "$CONFIG" ] || die "找不到配置文件 $CONFIG"

# ---------------------------------------------------------------------------
# 轮换冻结闸门：动手之前先说清「旧公钥是不是已经发出去了」
# ---------------------------------------------------------------------------
# 判据不靠人记：扫本地 tag 里各版本的 tauri.conf.json 有没有带上公钥（离线、确定性），
# 能连上 gh 时再合并线上 Release 的 tag（本地 tag 可能没 fetch 全）。
# 闸门在任何改动之前，被拦下时不会生成密钥、不会改配置、不会碰 secret。
gate="$(CONFIG="$CONFIG" ROOT="$ROOT" node -e '
  const { execFileSync, spawnSync } = require("node:child_process");
  const fs = require("node:fs");
  // minisign 的公钥载荷是 算法[2] | key id[8] | …，key id 是明文；配置与 .pub 里存的
  // 都是「minisign 原文的 base64」，所以要解开两层才拿得到 key id
  const keyIdOf = (payload) => (payload.length < 10
    ? ""
    : [...payload.subarray(2, 10)].reverse().map((byte) => byte.toString(16).padStart(2, "0")).join("").toUpperCase());
  const payloadOf = (text) => {
    const line = text
      .split("\n")
      .map((each) => each.trim())
      .filter(Boolean)
      .find((each) => !each.startsWith("untrusted comment") && !each.startsWith("trusted comment"));
    return line ? Buffer.from(line, "base64") : Buffer.alloc(0);
  };
  const keyIdOfConfig = (pubkey) => (pubkey ? keyIdOf(payloadOf(Buffer.from(pubkey, "base64").toString("utf8"))) : "");
  const keyIdOfJson = (json) => {
    try { return keyIdOfConfig(JSON.parse(json).plugins?.updater?.pubkey ?? ""); } catch { return ""; }
  };

  const current = keyIdOfJson(fs.readFileSync(process.env.CONFIG, "utf8"));

  let localTags = [];
  let localError = "";
  try {
    localTags = execFileSync("git", ["-C", process.env.ROOT, "tag", "--list", "--sort=v:refname"], { encoding: "utf8" })
      .split("\n").map((each) => each.trim()).filter(Boolean);
  } catch (err) { localError = err.message.split("\n")[0]; }

  const shipped = [];
  for (const tag of localTags) {
    let json = "";
    try {
      json = execFileSync("git", ["-C", process.env.ROOT, "show", `${tag}:src-tauri/tauri.conf.json`], { encoding: "utf8" });
    } catch { continue; }
    const keyId = keyIdOfJson(json);
    if (keyId) shipped.push(`${tag}[${keyId}]`);
  }

  const gh = spawnSync("gh", ["release", "list", "--limit", "100", "--json", "tagName", "-q", ".[].tagName"],
    { cwd: process.env.ROOT, encoding: "utf8", timeout: 20000 });
  const ghTags = gh.status === 0
    ? String(gh.stdout ?? "").split("\n").map((each) => each.trim()).filter(Boolean)
    : [];
  const ghUnchecked = ghTags.filter((tag) => !localTags.includes(tag));

  console.log(current);
  console.log(shipped.join(", "));
  console.log(shipped[0] ?? "");
  console.log(gh.status === 0 ? "gh" : "no-gh");
  console.log(ghUnchecked.join(", "));
  console.log(localError);
')" || die "冻结闸门自己跑失败了（读不出公钥/版本信息），已中止"

current_id="$(printf '%s\n' "$gate" | sed -n '1p')"
shipped_tags="$(printf '%s\n' "$gate" | sed -n '2p')"
first_shipped="$(printf '%s\n' "$gate" | sed -n '3p')"
gh_state="$(printf '%s\n' "$gate" | sed -n '4p')"
gh_unchecked="$(printf '%s\n' "$gate" | sed -n '5p')"
git_error="$(printf '%s\n' "$gate" | sed -n '6p')"

echo "• 当前公钥 key id（${CONFIG}）：${current_id:-（解不出，配置里可能还没有公钥）}"
if [ -n "$shipped_tags" ]; then
  echo "  ⛔ 已经有版本带着公钥发出去了：$shipped_tags"
  echo "     最早从 ${first_shipped%%\[*} 起 —— 这些客户端只认 key id ${current_id:-（解不出）}，轮换后它们"
  echo "     **永远收不到自动更新**（只能让用户手动重装一次）。"
else
  echo "  ✓ 本地 tag 里没有发现「已带公钥发布」的版本"
fi
if [ -n "$git_error" ]; then
  echo "  ⚠️ 读本地 tag 失败（${git_error}）—— 上面的判据不成立，请自己核对已发布版本"
fi
if [ "$gh_state" = "gh" ]; then
  if [ -n "$gh_unchecked" ]; then
    echo "  ⚠️ 线上还有本地没有的 tag（无法核对是否带公钥）：$gh_unchecked"
  else
    echo "  ✓ 顺带核对了线上 Release：没有本地缺的 tag"
  fi
else
  echo "  ⚠️ gh 不可用或未登录，只按本地 tag 判断 —— 请自己确认线上有没有已发布版本"
fi

if [ "${ALLOW_ROTATE:-}" = "1" ]; then
  echo "• ALLOW_ROTATE=1：放行轮换（视为你已确认接受「老用户手动重装一次」的后果）"
elif [ -t 0 ]; then
  echo "• 确认点：上面这些版本确实已经发到用户手上了吗？轮换不可撤回。"
  # read 在 EOF（Ctrl-D）时返回非零，而 set -e 会让脚本在这里直接退出、连中止原因都不打印，
  # 所以显式吞掉它的返回值，一律走下面这句 die
  rotate_confirm=""
  read -r -p "  确认轮换请输入 ROTATE（其它任何输入都会中止）: " rotate_confirm || true
  [ "$rotate_confirm" = "ROTATE" ] || die "已中止：没有生成密钥、没有改配置、没有动 secret"
  unset rotate_confirm
  echo "• 已确认，继续"
else
  die "非交互运行且未放行，已拦下（没有生成密钥、没有改配置、没有动 secret）。确认要轮换请加 ALLOW_ROTATE=1 重跑"
fi

# ---------------------------------------------------------------------------
# 密码
# ---------------------------------------------------------------------------
# 区分「没给」与「给了空字符串」：前者走交互，后者由下面的非空校验拦下并给出对应提示
if [ -n "${UPDATER_KEY_PASSWORD+set}" ]; then
  password="$UPDATER_KEY_PASSWORD"
else
  [ -t 0 ] || die "当前不是交互终端。请用 UPDATER_KEY_PASSWORD=... 传入密码，或先手动生成密钥"
  read -rsp "请输入新私钥密码（输入时不回显）: " password; echo
  read -rsp "再输入一次确认: " password_again; echo
  [ "$password" = "$password_again" ] || die "两次输入不一致"
  unset password_again
fi
[ -n "$password" ] || die "密码不能为空。要无密码私钥请直接用：tauri signer generate -w $KEY"
[ "${#password}" -ge 8 ] || echo "⚠️  密码只有 ${#password} 个字符，建议再长一些" >&2

# ---------------------------------------------------------------------------
# 生成新密钥（旧密钥先备份）
# ---------------------------------------------------------------------------
if [ -n "${SKIP_GENERATE:-}" ]; then
  [ -f "$KEY" ] || die "SKIP_GENERATE=1 但 $KEY 不存在"
  [ -f "$KEY.pub" ] || die "SKIP_GENERATE=1 但 $KEY.pub 不存在"
  echo "• 跳过生成，直接使用现有密钥 $KEY"
else
  if [ -f "$KEY" ]; then
    backup="$KEY.bak-$(date +%Y%m%d%H%M%S)"
    cp "$KEY" "$backup"
    [ -f "$KEY.pub" ] && cp "$KEY.pub" "$backup.pub"
    echo "• 旧密钥已备份到 ${backup}（旧公钥对应的客户端一旦发出去就不能再换密钥了，留着便于对照）"
  fi
  mkdir -p "$(dirname "$KEY")"
  rm -f "$KEY" "$KEY.pub"
  echo "• 生成新密钥对 → $KEY"
  (cd "$ROOT/src-tauri" && cargo tauri signer generate -w "$KEY" -p "$password" >/dev/null) \
    || die "生成失败（密码含特殊字符时注意 shell 转义；也可以手动跑 tauri signer generate 后加 SKIP_GENERATE=1）"
fi

[ -s "$KEY" ] || die "私钥文件为空：$KEY"
[ -s "$KEY.pub" ] || die "公钥文件为空：$KEY.pub"

# ---------------------------------------------------------------------------
# 公钥写回 tauri.conf.json（.pub 文件里就是配置要的那段 base64）
# ---------------------------------------------------------------------------
pubkey="$(tr -d '\n' < "$KEY.pub")"
case "$(printf '%s' "$pubkey" | base64 -d 2>/dev/null || true)" in
  'untrusted comment'*) ;;
  *) die "$KEY.pub 不是预期的 minisign 公钥（base64）格式，请确认生成命令" ;;
esac

echo "• 更新 $CONFIG 里的 plugins.updater.pubkey"
CONFIG="$CONFIG" PUBKEY="$pubkey" node -e '
  const fs = require("node:fs");
  const path = process.env.CONFIG;
  const config = JSON.parse(fs.readFileSync(path, "utf8"));
  const keyIdOf = (text) => {
    const line = text
      .split("\n")
      .map((each) => each.trim())
      .filter(Boolean)
      .find((each) => !each.startsWith("untrusted comment") && !each.startsWith("trusted comment"));
    if (!line) return "(解不出)";
    const payload = Buffer.from(line, "base64");
    return [...payload.subarray(2, 10)].reverse().map((b) => b.toString(16).padStart(2, "0")).join("").toUpperCase();
  };
  const before = config.plugins?.updater?.pubkey ?? "";
  config.plugins ??= {};
  config.plugins.updater ??= {};
  config.plugins.updater.pubkey = process.env.PUBKEY;
  fs.writeFileSync(path, `${JSON.stringify(config, null, 2)}\n`);
  const idOfB64 = (b64) => (b64 ? keyIdOf(Buffer.from(b64, "base64").toString("utf8")) : "(原本没有)");
  console.log(`  公钥 key id: ${idOfB64(before)} → ${idOfB64(process.env.PUBKEY)}`);
'

# ---------------------------------------------------------------------------
# CI secret：私钥内容 + 密码（都从 stdin 写，不进参数列表）
# ---------------------------------------------------------------------------
if [ -n "${SKIP_GH:-}" ]; then
  echo "• 跳过 CI secret 更新（SKIP_GH=1）"
else
  command -v gh >/dev/null 2>&1 || die "找不到 gh，无法更新 secret（可 SKIP_GH=1 只改本地）"
  repo="${GITHUB_REPOSITORY:-}"
  if [ -z "$repo" ]; then
    repo="$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null || true)"
  fi
  [ -n "$repo" ] || die "无法确定仓库名，请设置 GITHUB_REPOSITORY=owner/repo"
  echo "• 覆盖 $repo 的 secret：TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
  # 值都从 stdin 送（不进参数列表）；实测 gh 会原样存入，tauri 也容忍私钥末尾多一个换行
  gh secret set TAURI_SIGNING_PRIVATE_KEY --repo "$repo" < "$KEY" \
    || die "写入私钥 secret 失败（需要仓库 admin 权限）"
  printf '%s' "$password" | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo "$repo" \
    || die "写入密码 secret 失败"
  gh secret list --repo "$repo" | grep -E '^TAURI_SIGNING_PRIVATE_KEY' | sed 's/^/  /' || true
fi

# ---------------------------------------------------------------------------
# 自检：真签一次，判定私钥可用 + 与刚写进配置的公钥配对
# ---------------------------------------------------------------------------
echo "• 自检（真签一次并比对 key id）"
env -u TAURI_SIGNING_PRIVATE_KEY \
  TAURI_SIGNING_PRIVATE_KEY_PATH="$KEY" \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$password" \
  node "$ROOT/.github/scripts/check-signing-key.mjs" "$CONFIG" \
  || die "自检没通过 —— 现在还没到能发版的状态，请把上面的报错发出来"

cat <<EOF

✓ 密钥已轮换并自检通过。

接下来必须做的事：
  1. 提交 tauri.conf.json —— CI 用的是仓库里的公钥，不提交就等于「用新私钥签、让客户端按旧公钥验」，发版必被拦下；
       git add src-tauri/tauri.conf.json && git commit -m "chore: 轮换自动更新签名密钥"
  2. 把私钥与密码另存一份到安全的地方（密码忘了 = 私钥作废 = 之后再也发不出自动更新），
     旧的离线副本这次要一并作废、换成新私钥：
       私钥：$KEY
       密码：只有你知道，请存入密码管理器
       备份 / 恢复 / 自检流程见 DEVELOPMENT.md §8.9「私钥的备份与恢复」
  3. 打一个新 tag 发版（见 DEVELOPMENT.md §8.8/§8.9）：CI 会先校验这对密钥，再出带 latest.json 的 Release。
  4. 已经装过旧版本的客户端只认旧公钥，自动更新会静默失败 —— 得让那些用户手动重装一次
     （这就是轮换的代价，也是闸门要拦在前面的原因）。
EOF
