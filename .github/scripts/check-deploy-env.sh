#!/usr/bin/env bash
#
# check-deploy-env.sh — 发布流水线的快速预检（fast-fail）。
#
# 在多平台构建矩阵之前运行，让缺失/配错的部署凭据在几秒内失败，
# 而不是等几十分钟的构建跑完才暴露。检查内容：
#
#   1. 必需环境变量齐全且非空。
#   2. SSH 认证方式有且只有一种（私钥或密码）。
#   3. SSH 端口（若提供）是合法数字。
#   4. 目标主机可达且凭据可登录。
#   5. scp 试传一个临时文件并确认落地，随后清理。
#
# 用法：scripts/check-deploy-env.sh
# 成功退出 0，任何失败退出 1。本地和 CI 都可用。
set -euo pipefail

# ---------------------------------------------------------------------------
# 第 0 步 — 拒绝会破坏远端路径解析的字符。
# WEB_DEPLOY_PATH 会被包在单引号里传给 ssh/scp，混入单引号会悄悄
# 破坏 host:path 解析，所以大声报错。
# ---------------------------------------------------------------------------
if [[ "${WEB_DEPLOY_PATH:-}" == *"'"* ]]; then
  echo "::error::WEB_DEPLOY_PATH 包含单引号，拒绝继续: '$WEB_DEPLOY_PATH'" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 第 1 步 — 环境变量是否齐全。
# ---------------------------------------------------------------------------
declare -a MISSING=()
for var in SSH_HOST SSH_USERNAME WEB_DEPLOY_PATH; do
  if [[ -z "${!var:-}" ]]; then
    MISSING+=("$var")
  fi
done

if [[ ${#MISSING[@]} -gt 0 ]]; then
  echo "::error::缺少必需的环境变量: ${MISSING[*]}" >&2
  echo "::error::请在仓库 Settings → Secrets and variables → Actions 中配置。" >&2
  for var in "${MISSING[@]}"; do
    echo "::error::  - $var" >&2
  done
  exit 1
fi

# ---------------------------------------------------------------------------
# 第 2 步 — SSH 认证方式有且只有一种。
# ---------------------------------------------------------------------------
AUTH_METHODS=0
[[ -n "${SSH_PRIVATE_KEY:-}" ]] && AUTH_METHODS=$((AUTH_METHODS + 1))
[[ -n "${SSH_PASSWORD:-}" ]] && AUTH_METHODS=$((AUTH_METHODS + 1))

if [[ "$AUTH_METHODS" -eq 0 ]]; then
  echo "::error::未配置任何 SSH 认证方式。" >&2
  echo "::error::请在 SSH_PRIVATE_KEY、SSH_PASSWORD 中恰好配置一个。" >&2
  exit 1
fi
if [[ "$AUTH_METHODS" -gt 1 ]]; then
  echo "::error::配置了多种 SSH 认证方式。" >&2
  echo "::error::请在 SSH_PRIVATE_KEY、SSH_PASSWORD 中恰好配置一个。" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 第 3 步 — 校验 SSH 端口（若提供）。
# ---------------------------------------------------------------------------
PORT="${SSH_PORT:-22}"
if ! [[ "$PORT" =~ ^[0-9]+$ ]]; then
  echo "::error::SSH_PORT 必须是数字，当前为: '$PORT'" >&2
  exit 1
fi
if [[ "$PORT" -lt 1 || "$PORT" -gt 65535 ]]; then
  echo "::error::SSH_PORT 超出范围: $PORT" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 第 4 步 — 连通性 + 登录测试。
# ---------------------------------------------------------------------------
# 端口用 -o 传（不要用 -p）：小写 -p 在 scp 里是"保留时间戳"，
# 不是"端口"——会把端口号当成源文件吞掉。以下选项 ssh 和 scp 共用。
SSH_OPTS=(-o "Port=$PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -o BatchMode=yes)

DEST="$SSH_USERNAME@$SSH_HOST"
echo "==> 正在测试 SSH 连接 $DEST:$PORT ..." >&2

REMOTE_ECHO='1234-deploy-preflight-5678'
if [[ -n "${SSH_PRIVATE_KEY:-}" ]]; then
  KEY_FILE="$(mktemp)"
  trap 'rm -f "$KEY_FILE"' EXIT
  printf '%s\n' "$SSH_PRIVATE_KEY" > "$KEY_FILE"
  chmod 600 "$KEY_FILE"
  SSH_OPTS+=(-i "$KEY_FILE" -o IdentitiesOnly=yes)
fi

# 在远端跑一条无害命令。按调用方配置的认证方式（私钥文件，或 sshpass 密码）执行。
run_remote() {
  local host="$1"
  shift
  if [[ -n "${SSH_PASSWORD:-}" ]]; then
    # shellcheck disable=SC2034
    local SSHPASS
    SSHPASS="$SSH_PASSWORD" sshpass -e ssh "${SSH_OPTS[@]}" "$host" "$@"
  else
    ssh "${SSH_OPTS[@]}" "$host" "$@"
  fi
}

if ! OUT="$(run_remote "$DEST" "echo $REMOTE_ECHO" 2>&1)"; then
  echo "::error::SSH 登录失败: $DEST:$PORT" >&2
  echo "::error::$OUT" >&2
  exit 1
fi

if [[ "$OUT" != *"$REMOTE_ECHO"* ]]; then
  echo "::error::远端返回异常（期望 '$REMOTE_ECHO'）。" >&2
  echo "::error::$OUT" >&2
  exit 1
fi

if ! run_remote "$DEST" "mkdir -p '$WEB_DEPLOY_PATH'" >/dev/null 2>&1; then
  echo "::error::无法在 $DEST 上创建远端目录 '$WEB_DEPLOY_PATH'" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 第 5 步 — scp 试传。
# ---------------------------------------------------------------------------
# 部署步骤用 scp 推页面，它的选项集和 ssh 不同（比如端口是 -P 不是 -p），
# 所以 ssh 登录通过不保证 scp 命令行也有效。用一个一次性文件走一遍
# 相同的代码路径，结束后清理。
PROBE_LOCAL="$(mktemp)"
PROBE_REMOTE=".deploy-preflight-$REMOTE_ECHO"
printf 'preflight %s\n' "$DEST:$PORT" > "$PROBE_LOCAL"

run_scp() {
  if [[ -n "${SSH_PASSWORD:-}" ]]; then
    # shellcheck disable=SC2034
    local SSHPASS
    SSHPASS="$SSH_PASSWORD" sshpass -e scp "${SSH_OPTS[@]}" "$1" "$2"
  else
    scp "${SSH_OPTS[@]}" "$1" "$2"
  fi
}

SCP_OUT="$(run_scp "$PROBE_LOCAL" "$DEST:$WEB_DEPLOY_PATH/$PROBE_REMOTE" 2>&1)" && SCP_RC=0 || SCP_RC=$?
SCP_OK=0
if [[ "$SCP_RC" -eq 0 ]]; then
  if run_remote "$DEST" "test -f '$WEB_DEPLOY_PATH/$PROBE_REMOTE'" >/dev/null 2>&1; then
    SCP_OK=1
  else
    SCP_OUT="文件已上传但远端 test -f 校验失败"
  fi
fi
run_remote "$DEST" "rm -f '$WEB_DEPLOY_PATH/$PROBE_REMOTE'" >/dev/null 2>&1 || true
rm -f "$PROBE_LOCAL"

if [[ "$SCP_OK" -ne 1 ]]; then
  echo "::error::scp 传到 $DEST 的 '$WEB_DEPLOY_PATH' 失败（退出码 $SCP_RC）。" >&2
  echo "::error::部署步骤用完全相同的选项执行 scp；请确认这些选项对 scp（而不只是 ssh）合法。" >&2
  if [[ -n "$SCP_OUT" ]]; then
    echo "::error::scp 输出: $SCP_OUT" >&2
  fi
  exit 1
fi

echo "==> OK: 部署环境就绪（$DEST，端口 $PORT）。" >&2
