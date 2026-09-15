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
# ⚠️ 什么时候可以轮换：只有在**还没有任何客户端带着旧公钥发出去**之前。
# 客户端的公钥是编译进安装包的（tauri.conf.json → plugins.updater.pubkey），
# 旧公钥一旦随安装包发出去，换私钥就等于那些用户再也收不到自动更新（只能手动重装）。
# 本项目 v0.0.12 及更早都还没有更新模块，所以现在轮换是安全的；**v0.0.13 发布后就不要再换**。
#
# 用法：
#   bash scripts/rotate-signing-key.sh
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
  2. 把私钥与密码另存一份到安全的地方（密码忘了 = 私钥作废 = 之后再也发不出自动更新）：
       私钥：$KEY
       密码：只有你知道，请存入密码管理器
  3. 发布 v0.0.13（见 DEVELOPMENT.md §8.8/§8.9）：CI 会先校验这对密钥，再出带 latest.json 的 Release。
EOF
