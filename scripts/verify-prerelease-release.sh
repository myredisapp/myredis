#!/usr/bin/env bash
#
# 端到端核验：预发布 tag 出包 + 下载回来的产物过 Gatekeeper（DEVELOPMENT.md §2.5 E / §8.10）。
#
# 为什么用**预发布 tag** 做这件事：E 要验的是「导入证书 → 签名 → 公证 → staple」在真实流水线上
# 跑得通，而打正式 tag 会立刻占用 latest 别名（`releases/latest/download/latest.json`），
# 一旦验不过，用户点「检查更新」就会拿到半成品 —— 发出去的版本收不回。tag 里带 `-`
# （如 v0.0.17-beta.1）会被 release.yml 当预发布：产物照样出、CI 照样全跑，但不占 latest，
# 已装正式版的用户什么都收不到（见 §8.9「更新源」）。
#
# 这个脚本把 E 的四段动作串起来，并且**默认不动任何远端状态**（与 scripts/configure-apple-secrets.sh
# 同款：先演练、显式放行才真做）：
#   ① 前置检查：tag 形状 / 是否重名 / 目标提交是否已推 / 6 个 APPLE_* secret 齐不齐 /
#      目标提交上的 release.yml 有没有「预发布不部署网站」的守卫（见下）；
#   ② 打并推送预发布 tag（要 ALLOW_PUSH=1 或交互输入 PUSH），然后等流水线跑完，逐 job / 逐步骤报状态；
#   ③ 把产物真的下载回来验收：调 scripts/verify-macos-download.sh 判 dmg 里的 .app 能不能过
#      Gatekeeper（补 quarantine + spctl + stapler + 身份 / TeamIdentifier），并核验清单 latest.json；
#   ④ 核验「latest 别名没被顶掉」：当前正式版的 latest.json 版本号前后必须一致。
#
# ⚠️ 一个必须知道的坑：release.yml 的 `deploy-web` job 会把**当前 tag** 注入
# `web/index.html`（版本号 + 下载直链）再 scp 到线上。预发布 tag 一旦走到那一步，下载页就会把
# beta 当可用版本 —— 这正是 E 的收尾要避免的。所以 release.yml 里给 deploy-web 加了
# 「tag 含 `-` 就不部署」的守卫，本脚本的第 ① 步会在**目标提交上**复查这条守卫是否生效
# （工作流文件取自被 tag 的那个提交，不是本地工作区，所以要按提交查）。
#
# 用法：
#   bash scripts/verify-prerelease-release.sh                     # 默认：只做前置检查 + 打印计划，不动远端
#   bash scripts/verify-prerelease-release.sh                     # 交互：确认后输入 PUSH 才打 tag
#   ALLOW_PUSH=1 bash scripts/verify-prerelease-release.sh        # 非交互放行：打 tag → 等 CI → 验收
#   VERIFY_ONLY=1 bash scripts/verify-prerelease-release.sh       # 只验收已有 tag（不打 tag、不等 CI）
#   CLEANUP=1 bash scripts/verify-prerelease-release.sh           # 清理：删掉预发布 Release 与 tag（仅限预发布）
#   上面这些开关两种写法都认：写在命令前（推荐）或跟在脚本名后。
#     TAG=v0.0.17-beta.1   指定 tag（默认按最新正式版 +1 个 patch 拼 -beta.N，自动避开重名）
#     REF=<commit>         指定要 tag 的提交（默认 HEAD；必须是已推到远端的提交）
#     REPO=owner/name      目标仓库（默认按当前目录的 gh remote 推导）
#     WORK=...             产物下载目录（默认 $TMPDIR/myredis-e2e-<tag>，结束后保留 —— 人工首装要用它）
#     CLEAN_WORK=1         结束后删掉 WORK 目录
#     EXPECT_TEAM_ID=...   断言 TeamIdentifier（默认从 ~/.tauri/myredis-apple-signing.env 读，见 §2.5 C）
#     ALLOW_UNSIGNED=1     允许在 6 个 secret 不齐时也推 tag（只验流水线机制，**验不到签名**）
#     ALLOW_WEB_OVERWRITE=1 允许在守卫缺失的提交上推 tag（会把线上下载页改成 beta 版本）
#     NO_WAIT=1            推完 tag 就走（之后用 VERIFY_ONLY=1 回来验收）
#     WAIT_TIMEOUT=5400    等流水线的秒数上限（默认 90 分钟：universal 构建要几十分钟）
#     ALLOW_DELETE=1       CLEANUP 时的非交互放行
#     MANIFEST_BASE=...    清单地址的基址（默认 https://github.com/<repo>）；指向本地演练服务时
#                          可以在不碰远端的情况下把「清单 / latest 别名」这两段判据整条跑一遍
#
# 退出码：0 = 该做的都做成了；1 = 有判据没过 / 被拦下；2 = 用法或环境问题。
#
# 写法上只用 bash 3.2 就有的东西（macOS 自带的是 3.2，没有 declare -A）—— 与 scripts/ 下其它脚本一致。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

die() { echo "✗ $*" >&2; exit 1; }
env_die() { echo "✗ $*" >&2; exit 2; }
ok() { echo "  ✓ $*"; }
warn() { echo "  ! $*" >&2; }
step() { echo; echo "• $*"; }

# 开关写成 `NAME=值` 的环境变量前缀（推荐），也认「跟在脚本名后面」的写法 ——
# 与 scripts/configure-apple-secrets.sh 同款：写反位置不该是「静默没生效」
for arg in "$@"; do
  case "${arg}" in
    TAG=*|REF=*|REPO=*|WORK=*|CREEDS_FILE=*) export "${arg}" ;;
    CLEANUP=*|VERIFY_ONLY=*|ALLOW_PUSH=*|ALLOW_DELETE=*|ALLOW_UNSIGNED=*) export "${arg}" ;;
    ALLOW_WEB_OVERWRITE=*|EXPECT_TEAM_ID=*|NO_WAIT=*|WAIT_TIMEOUT=*|CLEAN_WORK=*) export "${arg}" ;;
    MANIFEST_BASE=*) export "${arg}" ;;
    *) die "不认识的参数：${arg}（用法见本脚本头部注释）" ;;
  esac
done

command -v gh >/dev/null 2>&1 || env_die "找不到 gh（GitHub CLI）。装一个：brew install gh，然后 gh auth login"
gh auth status >/dev/null 2>&1 || env_die "gh 未登录：先 gh auth login"

REPO="${REPO:-}"
if [ -z "${REPO}" ]; then
  REPO="$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null)" \
    || env_die "推导不出目标仓库（当前目录不是仓库 / remote 不认识）。显式给 REPO=<owner/name>"
fi

SECRETS=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID)
# 清单地址的基址：默认就是客户端读的那个公开地址；演练 / 镜像场景可以换成别的
# （路径形状不变：<基址>/releases/latest/download/latest.json 与 <基址>/releases/download/<tag>/latest.json）
MANIFEST_BASE="${MANIFEST_BASE:-https://github.com/${REPO}}"
CREEDS_FILE="${CREEDS_FILE:-$HOME/.tauri/myredis-apple-signing.env}"

# ---------------------------------------------------------------------------
# 小工具
# ---------------------------------------------------------------------------
# 读凭据文件里的一个值（与 configure-apple-secrets.sh 同款解析：只看「名字=值」的行）
value_of() {
  local want="$1" line key value
  [ -f "${CREEDS_FILE}" ] || return 1
  while IFS= read -r line || [ -n "${line}" ]; do
    case "${line}" in ''|'#'*) continue ;; esac
    key="${line%%=*}"
    [ "${key}" = "${want}" ] || continue
    value="${line#*=}"
    case "${value}" in
      \"*\") value="${value#\"}"; value="${value%\"}" ;;
      \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac
    printf '%s' "${value}"
    return 0
  done < "${CREEDS_FILE}"
  return 1
}

# 取一份 latest.json 的 version 字段（我们自己生成的清单，格式固定，用 sed 足够；
# 不用 gh 是为了走客户端取清单的**同一个公开地址**，顺带证明它匿名可读）
manifest_version() {
  curl -fsSL --max-time 20 "$1" 2>/dev/null \
    | sed -n 's/^  *"version": "\([^"]*\)".*/\1/p' | head -n 1
}

latest_alias_version() {  # 客户端真正读的那个地址（最新正式 Release 的附件）
  manifest_version "${MANIFEST_BASE}/releases/latest/download/latest.json"
}

latest_stable_tag() {
  gh api "repos/${REPO}/releases/latest" --jq .tag_name 2>/dev/null || true
}

release_exists() {
  gh release view "$1" --repo "${REPO}" --json tagName -q .tagName >/dev/null 2>&1
}

# 目标提交上的 release.yml 里，deploy-web 有没有「预发布不部署」的守卫。
# 判据：工作流文件（取自被 tag 的提交）中 deploy-web job 块里存在 job 级 if。
web_guard_present() {
  git show "$1:.github/workflows/release.yml" 2>/dev/null \
    | sed -n '/^  deploy-web:/,/^  [a-zA-Z][a-zA-Z0-9_-]*:/p' \
    | grep -q '^ *if:'
}

# ---------------------------------------------------------------------------
# 1. 清理模式（CLEANUP=1）：删掉预发布 Release 与 tag —— 只对预发布动手
# ---------------------------------------------------------------------------
# 放在最前面：清理时不需要目标提交、不需要 secret，也不该被「tag 重名」这类前置检查拦住
if [ "${CLEANUP:-0}" = "1" ]; then
  if [ -z "${TAG:-}" ]; then
    # 没显式给 TAG 时，目标是**已经存在的**那个预发布（最近一个），不是推导出来的新 tag 名
    TAG="$(gh release list --repo "${REPO}" --limit 30 --json tagName,isPrerelease \
      --jq '[.[] | select(.isPrerelease)][0].tagName // ""' 2>/dev/null || true)"
  fi
  [ -n "${TAG}" ] || die "远端没有预发布 Release 可清理（也可以显式给 TAG=<预发布 tag>）"
  release_exists "${TAG}" || die "远端没有 ${TAG} 的 Release"

  IS_PRE="$(gh release view "${TAG}" --repo "${REPO}" --json isPrerelease -q .isPrerelease 2>/dev/null || echo "unknown")"
  [ "${IS_PRE}" = "true" ] \
    || die "${TAG} 在远端不是预发布（isPrerelease=${IS_PRE}）—— 本脚本只清理预发布，绝不删正式版"

  step "清理 ${TAG}"
  echo "  将删除（不可撤回）："
  echo "    · Release ${TAG} 的全部附件，以及远端 tag（gh release delete --cleanup-tag）"
  echo "    · 本地 tag（若存在）与产物目录 ${WORK:-${TMPDIR:-/tmp}/myredis-e2e-${TAG}}"
  echo "  Actions 的历史运行不受影响：gh run list --workflow release.yml --branch ${TAG} 仍可查"
  echo "  已装过这个 beta 的机器不会变成孤儿：版本号排在下一个正式版之后，收到正式版会正常更新"
  if [ "${ALLOW_DELETE:-0}" != "1" ]; then
    [ -t 0 ] || die "非交互路径必须显式放行：ALLOW_DELETE=1 bash scripts/verify-prerelease-release.sh CLEANUP=1"
    printf '确认删除？输入 DELETE 继续: ' >&2
    IFS= read -r answer
    [ "${answer}" = "DELETE" ] || die "已取消（没有删除任何东西）"
  fi

  gh release delete "${TAG}" --repo "${REPO}" --yes --cleanup-tag >/dev/null \
    || die "删除 Release 失败（仓库开了 Release Immutability？见 gh release delete --help）"
  ok "已删除 Release ${TAG} 与远端 tag"
  if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null 2>&1; then
    git tag -d "${TAG}" >/dev/null
    ok "已删除本地 tag"
  fi
  rm -rf "${WORK:-${TMPDIR:-/tmp}/myredis-e2e-${TAG}}"
  ok "已删除产物目录"

  NOW_VERSION="$(latest_alias_version || true)"
  if [ -n "${NOW_VERSION}" ]; then
    ok "latest 别名现在指向 version ${NOW_VERSION}（清理没有动正式版）"
  else
    warn "取不到 latest 别名 —— 手工确认：curl -fsSL ${MANIFEST_BASE}/releases/latest/download/latest.json"
  fi
  exit 0
fi

# ---------------------------------------------------------------------------
# 2. 两条路径共用的准备：latest 基准（核验前后必须一致）、期望的 TeamIdentifier、
#    产物目录与清单地址 —— 都是只读或本地动作，放这里省得两条路径各写一遍
# ---------------------------------------------------------------------------
VERIFY_ONLY="${VERIFY_ONLY:-0}"

if [ "${VERIFY_ONLY}" = "1" ] && [ -z "${TAG:-}" ]; then
  # 只验收又没给 TAG：取**远端最近一个预发布**（不能像下面那样推导一个新名字 —— 那个还不存在）
  TAG="$(gh release list --repo "${REPO}" --limit 30 --json tagName,isPrerelease \
    --jq '[.[] | select(.isPrerelease)][0].tagName // ""' 2>/dev/null || true)"
  [ -n "${TAG}" ] || die "远端没有预发布 Release 可验收；也可以显式给 TAG=<预发布 tag>"
  ok "自动选定的待验收 tag：${TAG}（远端最近一个预发布）"
fi
if [ "${VERIFY_ONLY}" = "1" ]; then
  WORK="${WORK:-${TMPDIR:-/tmp}/myredis-e2e-${TAG}}"
  VERIFY_URL="${MANIFEST_BASE}/releases/download/${TAG}/latest.json"
fi

ANCHOR_TAG="$(latest_stable_tag)"
ANCHOR_VERSION="$(latest_alias_version || true)"
if [ -n "${ANCHOR_TAG}" ] && [ -n "${ANCHOR_VERSION}" ]; then
  ok "当前的 latest 别名指向 ${ANCHOR_TAG}（清单 version ${ANCHOR_VERSION}）—— 核验完必须还是它"
else
  warn "取不到当前 latest 别名（${ANCHOR_TAG:-无 tag} / ${ANCHOR_VERSION:-无 version}）—— 前后比对会跳过"
fi
EXPECT_TEAM_ID="${EXPECT_TEAM_ID:-$(value_of APPLE_TEAM_ID || true)}"

# 下面到「打印计划」为止，都是**要推 tag** 的那条路才需要的检查（VERIFY_ONLY=1 时整段跳过：
# 那些检查问的是「能不能推这个新 tag」，而只验收的场景里 tag 早就推出去了）
if [ "${VERIFY_ONLY}" != "1" ]; then

# ---------------------------------------------------------------------------
# 3. 目标提交：必须是已推到远端的提交（release.yml 是被 tag 的那个提交上的版本）
# ---------------------------------------------------------------------------
REF="${REF:-HEAD}"
SHA="$(git rev-parse --verify "${REF}^{commit}" 2>/dev/null)" || env_die "解析不出提交：${REF}"
# 末尾的 `|| true` 不是装饰：提交不在任何远端分支上时，grep 一行都没选中 → 退出码 1，
# 在 `set -e` + pipefail 下这个「命令替换」会被判为失败，脚本就在这里**静默退出**，
# 连下面那句「还没推到远端」的理由都打不出来（这个坑是本地演练里真踩出来的）。
BRANCH_OF_SHA="$(git branch -r --contains "${SHA}" 2>/dev/null | grep -v 'HEAD ->' | sed -n 's#^ *origin/##p' | head -n 1 | tr -d ' ' || true)"
if [ -n "${BRANCH_OF_SHA}" ]; then
  ok "目标提交 ${SHA:0:8}（在远端分支 ${BRANCH_OF_SHA} 上）"
else
  die "目标提交 ${SHA:0:8} 还没推到远端 —— release.yml 在 runner 上 checkout 这个 tag，提交必须在远端。
   先推分支（或显式给一个已推的 REF=<commit>）"
fi
if ! git diff-index --quiet HEAD -- 2>/dev/null; then
  warn "工作区有未提交改动 —— tag 打的是提交，未提交的改动不会进这次核验"
fi

# ---------------------------------------------------------------------------
# 4. tag：形状必须是「预发布」（含 `-`），且不能重名
# ---------------------------------------------------------------------------
if [ -z "${TAG:-}" ]; then
  BASE="$(latest_stable_tag)"
  [ -n "${BASE}" ] || env_die "取不到最新正式 Release（gh api 失败），没法推导 tag 名。显式给 TAG=<预发布 tag>"
  # 版本号 +1 个 patch：预发布是「下一个正式版」的候选，数字上必须排在当前正式版之后
  case "${BASE}" in
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *) env_die "最新正式 tag「${BASE}」不是 vX.Y.Z 形式，没法推导下一个版本号。显式给 TAG=<预发布 tag>" ;;
  esac
  NUM="${BASE#v}"
  MAJOR="${NUM%%.*}"; REST="${NUM#*.}"; MINOR="${REST%%.*}"; PATCH="${REST#*.}"
  for part in "${MAJOR}" "${MINOR}" "${PATCH}"; do
    case "${part}" in
      ''|*[!0-9]*) env_die "最新正式 tag「${BASE}」的版本号里有非数字段，没法推导。显式给 TAG=<预发布 tag>" ;;
    esac
  done
  N=1
  while :; do
    CANDIDATE="v${MAJOR}.${MINOR}.$((PATCH + 1))-beta.${N}"
    if git rev-parse -q --verify "refs/tags/${CANDIDATE}" >/dev/null 2>&1 \
      || [ -n "$(git ls-remote --tags --refs origin "refs/tags/${CANDIDATE}" 2>/dev/null)" ] \
      || release_exists "${CANDIDATE}"; then
      N=$((N + 1))
      continue
    fi
    TAG="${CANDIDATE}"
    break
  done
  ok "自动选定的预发布 tag：${TAG}（在最新正式版 ${BASE} 的 patch 上 +1）"
fi

# shape：与 .github/scripts/set-version.mjs、gen-latest-json.mjs、rename-artifacts.mjs 的校验对齐
case "${TAG}" in
  v[0-9]*.[0-9]*.[0-9]*-[0-9A-Za-z.-]*) ;;
  *) die "tag「${TAG}」不合法：预发布必须是 vX.Y.Z-<后缀> 形式（如 v0.0.17-beta.1）。
   不带 - 的 tag 会占用 latest 别名（已装用户会直接收到这个包），不能用来做核验" ;;
esac
case "${TAG}" in
  *[!0-9A-Za-z._-]*) die "tag「${TAG}」含非法字符（只允许 0-9 A-Za-z . _ -）：文件名 / URL / sed 都靠它" ;;
esac

step "预发布 tag：${TAG} → 提交 ${SHA:0:8}"

# ---------------------------------------------------------------------------
# 5. 前置检查：重名、runner 依赖的凭据、以及「预发布会不会把线上下载页改掉」
# ---------------------------------------------------------------------------
PREREQ_FAILED=0
prereq_bad() { echo "  ✗ $*" >&2; PREREQ_FAILED=$((PREREQ_FAILED + 1)); }

if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null 2>&1; then
  prereq_bad "本地已有同名 tag ${TAG}（要重跑请先 git tag -d ${TAG}，或换一个 tag 名）"
else
  ok "本地没有同名 tag"
fi
if [ -n "$(git ls-remote --tags --refs origin "refs/tags/${TAG}" 2>/dev/null)" ]; then
  prereq_bad "远端已有同名 tag ${TAG}（tag 指向的提交不可改：换 tag 名，或先删远端的）"
else
  ok "远端没有同名 tag"
fi
if release_exists "${TAG}"; then
  # 重跑同一 tag 时 release.yml 走的是 upload --clobber 分支，产物会被覆盖；
  # 但「本金已发出去」这种事不该由这个脚本悄悄决定
  prereq_bad "已存在同名 Release ${TAG}（重跑会覆盖它的产物 —— 确认这是你要的，或用 VERIFY_ONLY=1 只验收）"
else
  ok "远端没有同名 Release"
fi

if web_guard_present "${SHA}"; then
  ok "目标提交上的 release.yml 带「预发布不部署网站」的守卫（线上下载页不会被改成 beta 版本）"
else
  if [ "${ALLOW_WEB_OVERWRITE:-0}" = "1" ]; then
    warn "目标提交上的 deploy-web 没有预发布守卫，但 ALLOW_WEB_OVERWRITE=1 放行了 —— 线上下载页会被改成 ${TAG}"
  else
    prereq_bad "目标提交上的 release.yml 里，deploy-web 没有「tag 含 - 就不部署」的守卫：
   预发布跑完会把线上下载页（版本号 + 下载直链）改成 ${TAG}，等于把 beta 当可用版本。
   要么先把这个守卫合进该提交，要么显式传 ALLOW_WEB_OVERWRITE=1 接受这个后果"
  fi
fi

secrets_group_ok() {  # 6 个 secret 齐不齐（CI 里 setup-macos-signing.sh 也是成组校验）
  local present name count=0
  present="$(gh secret list --repo "${REPO}" --json name -q '.[].name' 2>/dev/null)" \
    || { warn "读不出 secret 列表（权限不够？需要仓库 admin）：gh secret list --repo ${REPO}"; return 1; }
  for name in "${SECRETS[@]}"; do
    printf '%s\n' "${present}" | grep -qx "${name}" && count=$((count + 1))
  done
  SECRET_COUNT="${count}"
  [ "${count}" -eq "${#SECRETS[@]}" ]
}

SECRET_COUNT=0
if secrets_group_ok; then
  ok "6 个 APPLE_* secret 都在（CI 会走「签名 + 公证」这条路径）"
else
  if [ "${ALLOW_UNSIGNED:-0}" = "1" ]; then
    warn "只有 ${SECRET_COUNT}/6 个 APPLE_* secret，但 ALLOW_UNSIGNED=1 放行了：
   这次只能验流水线机制（产物照样出），**验不到签名 / 公证** —— 下面的判据会如实报失败"
  else
    prereq_bad "APPLE_* secret 只有 ${SECRET_COUNT}/6 个 —— 这样跑出来的产物是未签名的，
   「Gatekeeper 实测」这一趟等于白跑（CI 只会在日志里告警，不会失败）。
   先备齐并配置凭据（bash scripts/prepare-apple-signing.sh → ALLOW_CONFIGURE=1 bash scripts/configure-apple-secrets.sh），
   或者显式传 ALLOW_UNSIGNED=1 表示「只验流水线机制」"
  fi
fi

if [ "${PREREQ_FAILED}" -gt 0 ]; then
  echo
  die "前置检查有 ${PREREQ_FAILED} 项没过（见上）—— 没有打 tag、没有动任何远端状态"
fi

# ---------------------------------------------------------------------------
# 6. 打印计划（默认走到这里就结束：不动远端）
# ---------------------------------------------------------------------------
WORK="${WORK:-${TMPDIR:-/tmp}/myredis-e2e-${TAG}}"
VERIFY_URL="${MANIFEST_BASE}/releases/download/${TAG}/latest.json"

plan() {
  cat <<EOF

• 接下来会做什么（真实动作，都会动远端）：
    1) git tag ${TAG} ${SHA:0:8} && git push origin refs/tags/${TAG}
       → 触发 release.yml：质量门禁 / 部署环境校验 → 三平台构建（macOS 走「导入证书 → 签名 → 公证 → staple」）
         → 建预发布 Release（不占 latest）→ 预发布不部署网站
    2) 等流水线跑完，逐 job 报状态，并确认 macOS 的「校验 macOS 签名与公证」这一步是 success
    3) 把产物下载到 ${WORK} 并验收：
         bash scripts/verify-macos-download.sh <dmg> EXPECT_TEAM_ID=${EXPECT_TEAM_ID:-<未指定>}
       （补 quarantine 后判 codesign / 身份 / TeamIdentifier / spctl / stapler，判 dmg 里的那个 .app）
    4) 核验清单与别名：
         - ${VERIFY_URL} 可匿名取到，version = ${TAG#v}，4 个平台键都指向本 tag 的产物
         - latest 别名仍是 ${ANCHOR_TAG:-（未取到）}（version ${ANCHOR_VERSION:-?}）：预发布没顶掉正式版
    5) 打印剩下的人工动作（干净 macOS 上双击首装）与清理 / 标注方式

• 想真跑：ALLOW_PUSH=1 bash scripts/verify-prerelease-release.sh（或交互输入 PUSH）
  只想先看流水线状态：NO_WAIT=1 ...
  已经跑过一遍、只想回来验收：VERIFY_ONLY=1 bash scripts/verify-prerelease-release.sh
EOF
}

  plan
  if [ "${ALLOW_PUSH:-0}" != "1" ]; then
    if [ -t 0 ]; then
      printf '确认推这个预发布 tag？输入 PUSH 继续: ' >&2
      IFS= read -r answer
      [ "${answer}" = "PUSH" ] || die "已取消（没有打 tag、没有动远端）"
    else
      cat <<EOF

• 非交互环境：到这里就结束了，**什么都没有做**（默认行为就是演练）。
  要真跑：ALLOW_PUSH=1 bash scripts/verify-prerelease-release.sh
EOF
      exit 0
    fi
  else
    echo
    echo "• ALLOW_PUSH=1：放行（视为确认「用预发布 tag 跑一遍真实流水线」）"
  fi

  step "打并推送预发布 tag"
  git tag "${TAG}" "${SHA}"
  if git push origin "refs/tags/${TAG}"; then
    ok "已推送 ${TAG}（提交 ${SHA:0:8}）"
  else
    die "推送 tag 失败。本地 tag 还在（git tag -d ${TAG} 可撤销），远端状态未变"
  fi

  if [ "${NO_WAIT:-0}" = "1" ]; then
    cat <<EOF

• NO_WAIT=1：不等流水线了。之后用下面的命令回来看状态与验收：
    VERIFY_ONLY=1 bash scripts/verify-prerelease-release.sh
  看这一次的运行：gh run list --repo ${REPO} --workflow release.yml --branch ${TAG}
EOF
    exit 0
  fi

fi   # ← 「要推 tag」那条路到此结束；VERIFY_ONLY=1 从上面直接跳到验收

# ---------------------------------------------------------------------------
# 7. 等流水线（VERIFY_ONLY 模式跳过：只验收已有产物）
# ---------------------------------------------------------------------------
RUN_ID=""
if [ "${VERIFY_ONLY:-0}" != "1" ]; then
  step "等 release.yml 跑完（上限 $(( ${WAIT_TIMEOUT:-5400} / 60 )) 分钟）"
  DEADLINE=$(( $(date +%s) + ${WAIT_TIMEOUT:-5400} ))
  STATUS=""
  CONCLUSION=""
  while :; do
    RUN_LINE="$(gh run list --repo "${REPO}" --workflow release.yml --branch "${TAG}" --limit 1 \
      --json databaseId,status,conclusion --jq '.[] | "\(.databaseId) \(.status) \(.conclusion // "-")"' 2>/dev/null | head -n 1 || true)"
    RUN_ID="${RUN_LINE%% *}"
    REST="${RUN_LINE#* }"
    STATUS="${REST%% *}"
    CONCLUSION="${REST#* }"
    if [ -z "${RUN_ID}" ]; then
      echo "  还没看到 ${TAG} 的运行（刚推上去要几秒才出现）…"
    elif [ "${STATUS}" = "completed" ]; then
      break
    else
      echo "  ${TAG} 运行 ${RUN_ID}：${STATUS}（已等 $(( $(date +%s) - (DEADLINE - ${WAIT_TIMEOUT:-5400}) ))s）"
    fi
    if [ "$(date +%s)" -gt "${DEADLINE}" ]; then
      warn "等超时（WAIT_TIMEOUT=$(( ${WAIT_TIMEOUT:-5400} / 60 )) 分钟）—— 打包仍在继续，可用 VERIFY_ONLY=1 回来验收"
      echo "  看状态：gh run view ${RUN_ID} --repo ${REPO}"
      exit 1
    fi
    sleep 20
  done

  step "流水线状态（run ${RUN_ID}）"
  gh run view "${RUN_ID}" --repo "${REPO}" \
    --json jobs --jq '.jobs[] | "  \(.name) => \(.conclusion // "?")"' || true

  SIGNING_STEP="$(gh run view "${RUN_ID}" --repo "${REPO}" --json jobs \
    --jq '[.jobs[] | select(.name | startswith("构建 macos")) | .steps[] | select(.name | contains("校验 macOS 签名与公证")) | .conclusion] | first // ""' 2>/dev/null || true)"
  if [ "${SIGNING_STEP}" = "success" ]; then
    ok "macOS 的「校验 macOS 签名与公证」这一步是 success（E 验收 ① 的 CI 侧）"
  else
    warn "macOS 的「校验 macOS 签名与公证」这一步不是 success（读到「${SIGNING_STEP:-空}」）。
   注意：这一步在「没凭据」时也会 success（只打告警）—— 所以「有没有真签名」一律以下面的产物判据为准"
  fi

  if [ "${CONCLUSION}" != "success" ]; then
    echo
    echo "✗ 流水线结论是 ${CONCLUSION}：看失败原因：gh run view ${RUN_ID} --repo ${REPO} --log-failed" >&2
    echo "  本次 Release 可能没建出来（或只有部分产物）。" >&2
    if [ -n "${ANCHOR_VERSION}" ]; then
      NOW="$(latest_alias_version || true)"
      if [ "${NOW}" = "${ANCHOR_VERSION}" ]; then
        echo "  ✓ latest 别名未受影响（仍是 ${ANCHOR_VERSION}）—— 已装用户不会收到这个半成品" >&2
      else
        echo "  ✗ latest 别名现在是「${NOW}」（原为 ${ANCHOR_VERSION}）—— 立刻查：gh release list --repo ${REPO} --limit 5" >&2
      fi
    fi
    echo "  这个预发布可以留着排查，或清理掉：CLEANUP=1 bash scripts/verify-prerelease-release.sh" >&2
    exit 1
  fi
  ok "三平台构建 + 建 Release + 清单回读 全部 success"
fi

# ---------------------------------------------------------------------------
# 8. 验收：先确认这是预发布，再把产物下载回来判
# ---------------------------------------------------------------------------
step "验收 ${TAG}"

RELEASE_JSON="$(gh release view "${TAG}" --repo "${REPO}" --json tagName,isPrerelease,assets \
  -q '"\(.tagName) \(.isPrerelease) \([.assets[].name] | join("\n"))"' 2>/dev/null)" \
  || die "远端没有 ${TAG} 的 Release（流水线没跑到建 Release 这一步？）"

IS_PRE_RELEASE="$(printf '%s\n' "${RELEASE_JSON}" | head -n 1 | cut -d' ' -f2)"
ASSET_NAMES="$(printf '%s\n' "${RELEASE_JSON}" | tail -n +2)"
# 安全闸门：这个脚本的所有动作（下载、CLEANUP 删除）都只对预发布做 ——
# 万一 tag 名没带 `-` 或被人工改成了正式 Release，绝不能顺着往下走
[ "${IS_PRE_RELEASE}" = "true" ] \
  || die "${TAG} 在远端不是预发布（isPrerelease=${IS_PRE_RELEASE}）—— 本脚本只处理预发布，已停下"
ok "远端确认是预发布（isPrerelease=true）"

mkdir -p "${WORK}"
DMG_ASSET="myredis-${TAG}-macos-universal.dmg"
printf '%s\n' "${ASSET_NAMES}" | grep -qx "${DMG_ASSET}" \
  || die "预发布里没有 ${DMG_ASSET}（有的附件：$(printf '%s ' ${ASSET_NAMES} | tr '\n' ' ')）—— macOS 构建没出这个包？"
ok "附件齐全度：dmg 在（共 $(printf '%s\n' "${ASSET_NAMES}" | grep -c .) 个附件）"

step "下载产物到 ${WORK}"
DMG_PATH="${WORK}/${DMG_ASSET}"
if [ -f "${DMG_PATH}" ]; then
  ok "已存在，跳过下载（要重新下就删掉它）：${DMG_PATH}"
else
  gh release download "${TAG}" --repo "${REPO}" --pattern "${DMG_ASSET}" --dir "${WORK}" \
    || die "下载 ${DMG_ASSET} 失败"
  ok "下载完成：$(du -h "${DMG_PATH}" | cut -f1)"
fi

VERIFY_EXIT=0
# 开关要走环境变量前缀：verify-macos-download.sh 只认一个位置参数（要判的产物）
EXPECT_TEAM_ID="${EXPECT_TEAM_ID}" EXPECT_VERSION="${TAG#v}" \
  bash "${ROOT}/scripts/verify-macos-download.sh" "${DMG_PATH}" || VERIFY_EXIT=$?

# ---------------------------------------------------------------------------
# 9. 清单与 latest 别名：预发布要「自己的清单可取」，同时「没顶掉正式版」
# ---------------------------------------------------------------------------
step "核验更新清单与 latest 别名"

MANIFEST="$(curl -fsSL --max-time 20 "${VERIFY_URL}" 2>/dev/null || true)"
if [ -z "${MANIFEST}" ]; then
  warn "取不到 ${VERIFY_URL} —— 本预发布里缺少 latest.json 附件（正式版会因此断掉自动更新）"
  VERIFY_EXIT=1
else
  ok "本 tag 的 latest.json 可匿名取到（${VERIFY_URL}）"
  M_VERSION="$(printf '%s' "${MANIFEST}" | sed -n 's/^  *"version": "\([^"]*\)".*/\1/p' | head -n 1 || true)"
  if [ "${M_VERSION}" = "${TAG#v}" ]; then
    ok "清单 version = ${M_VERSION}（与 tag 一致）"
  else
    warn "清单 version 是「${M_VERSION}」，期望「${TAG#v}」"
    VERIFY_EXIT=1
  fi
  # 4 个平台键都要在，且下载地址必须指向本 tag 的产物 —— 地址拼错时 /latest 地址在正式版上照样
  # 会 404，属于「清单发出来了但客户端下不到」的静默失败
  MISSING_KEYS=""
  for key in darwin-aarch64 darwin-x86_64 linux-x86_64 windows-x86_64; do
    printf '%s' "${MANIFEST}" | grep -q "\"${key}\"" || MISSING_KEYS="${MISSING_KEYS}${key} "
  done
  if [ -z "${MISSING_KEYS}" ]; then
    ok "4 个平台键都在（darwin-aarch64 / darwin-x86_64 / linux-x86_64 / windows-x86_64）"
  else
    warn "清单缺平台键：${MISSING_KEYS}"
    VERIFY_EXIT=1
  fi
  MANIFEST_ASSETS="$(printf '%s' "${MANIFEST}" | sed -n 's#.*/releases/download/[^/]*/\([^"]*\)".*#\1#p' | sort -u || true)"
  MISSING_ASSET=""
  for asset in ${MANIFEST_ASSETS}; do
    printf '%s\n' "${ASSET_NAMES}" | grep -qx "${asset}" || MISSING_ASSET="${MISSING_ASSET}${asset} "
  done
  if [ -z "${MISSING_ASSET}" ]; then
    ok "清单指向的产物都在 Release 里（链接不会 404）"
  else
    warn "清单指向了 Release 里没有的产物：${MISSING_ASSET}"
    VERIFY_EXIT=1
  fi
fi

if [ -n "${ANCHOR_VERSION}" ]; then
  NOW_VERSION="$(latest_alias_version || true)"
  if [ "${NOW_VERSION}" = "${ANCHOR_VERSION}" ]; then
    ok "latest 别名仍是 ${ANCHOR_TAG}（version ${ANCHOR_VERSION}）—— 预发布没顶掉正式版，已装用户收不到这个包"
  elif [ -z "${NOW_VERSION}" ]; then
    warn "取不到 latest 别名（网络？）—— 手工确认：curl -fsSL ${MANIFEST_BASE}/releases/latest/download/latest.json"
    VERIFY_EXIT=1
  else
    warn "latest 别名变成了 ${NOW_VERSION}（原为 ${ANCHOR_VERSION}）—— 正式版的自动更新源被改动了，立刻查：
   gh release list --repo ${REPO} --limit 5"
    VERIFY_EXIT=1
  fi
fi

# ---------------------------------------------------------------------------
# 10. 收尾
# ---------------------------------------------------------------------------
step "剩下的人工动作（脚本替代不了）"
cat <<EOF
  E 验收 ②：在一台**没装过本应用**的 macOS 上双击首装
    · 安装包：${DMG_PATH}
    · 判据：不再出现「无法验证开发者」，**不需要右键「打开」**
    · 想更接近「用户下载」的路径：用浏览器从
      https://github.com/${REPO}/releases/download/${TAG}/${DMG_ASSET} 下载一次再双击
  （脚本已经把 Gatekeeper 自己的判据——spctl / stapler / 身份 / TeamIdentifier——在带 quarantine
    属性的这份 dmg 与里面的 .app 上判过一遍：见上。人工这一步验的是「真机首装的体验」。）

  收尾（二选一）：
    · 保留并标注：Release 正文开头已经自动带上「内部验证用」提示，Releases 列表里也标着 Pre-release ——
      默认就这样留着，便于以后回看证据（run ${RUN_ID:-（无）}）
    · 删掉：CLEANUP=1 bash scripts/verify-prerelease-release.sh
  （下载页不受影响：release.yml 的 deploy-web 对带 \`-\` 的 tag 直接跳过，线上仍是上一个正式版）
EOF

if [ "${CLEAN_WORK:-0}" = "1" ]; then
  rm -rf "${WORK}"
  echo "• CLEAN_WORK=1：已删掉 ${WORK}"
fi

if [ "${VERIFY_EXIT}" -eq 0 ]; then
  echo
  echo "✓ 自动化部分全过：签名 / 公证 / Gatekeeper 判据 + 清单 + latest 别名都对"
  exit 0
fi
echo
echo "✗ 自动化部分有判据没过（逐条见上）—— 人工首装就别急着做了，先按上面的原因修" >&2
exit 1
