#!/system/bin/sh
# 参考依据:
# - lanvault_complete_blueprint.md 第 3.4 / 5.5 节 (Level 3 Magisk/KernelSU late_start 守护脚本)
# - 等待 sys.boot_completed=1 后初始化默认配置并单实例守护 lanvault-server，含 5MB 日志轮转、停服哨兵与异常退避

MODDIR="${0%/*}"
BIN_PATH="$MODDIR/bin/lanvault-server"
DATA_DIR="/data/adb/lanvault"
VAULT_DIR="$DATA_DIR/vault"
CONFIG_PATH="$DATA_DIR/config.toml"
PID_FILE="$DATA_DIR/server.pid"
WATCHDOG_PID_FILE="$DATA_DIR/watchdog.pid"
STOP_SENTINEL="$DATA_DIR/.stop_requested"
LOG_FILE="$DATA_DIR/daemon.log"
MAX_LOG_BYTES=5242880

while [ "$(getprop sys.boot_completed)" != "1" ]; do
    sleep 2
done

mkdir -p "$DATA_DIR" "$VAULT_DIR"

# 单实例防重入检查：若看门狗或服务端已在运行则直接退出
if [ -f "$WATCHDOG_PID_FILE" ]; then
    OLD_WD_PID="$(cat "$WATCHDOG_PID_FILE" 2>/dev/null)"
    if [ -n "$OLD_WD_PID" ] && [ "$OLD_WD_PID" != "$$" ] && [ -d "/proc/$OLD_WD_PID" ]; then
        exit 0
    fi
fi
echo "$$" > "$WATCHDOG_PID_FILE"
trap 'rm -f "$WATCHDOG_PID_FILE"' EXIT

if [ -f "$PID_FILE" ]; then
    OLD_SRV_PID="$(cat "$PID_FILE" 2>/dev/null)"
    if [ -n "$OLD_SRV_PID" ] && [ -d "/proc/$OLD_SRV_PID" ]; then
        exit 0
    fi
    rm -f "$PID_FILE"
fi

generate_random_password() {
    pw=""
    if [ -r /proc/sys/kernel/random/uuid ]; then
        pw="$(cat /proc/sys/kernel/random/uuid 2>/dev/null | tr -d '-')$(cat /proc/sys/kernel/random/uuid 2>/dev/null | tr -d '-')"
        pw="$(printf '%s' "$pw" | cut -c1-24)"
    fi
    if [ "${#pw}" -lt 24 ] && [ -r /dev/urandom ]; then
        pw="$(od -An -N12 -tx1 /dev/urandom 2>/dev/null | tr -d ' \n' | cut -c1-24)"
    fi
    printf '%s' "$pw"
}

# 首次缺少配置文件时自动生成带 24 位随机高熵密码的默认 config.toml
if [ ! -f "$CONFIG_PATH" ]; then
    GENERATED_PASSWORD="$(generate_random_password)"
    cat <<EOF > "$CONFIG_PATH"
bind_addr = "0.0.0.0"
port = 7842
vault_root = "/data/adb/lanvault/vault"
default_quota_bytes = 107374182400
web_password = "$GENERATED_PASSWORD"
device_name = "EveryoneNAS-Android-Daemon"
keep_alive_level = "LEVEL_3_MAGISK_DAEMON"
EOF
    chmod 600 "$CONFIG_PATH" 2>/dev/null || true
fi

rotate_log_if_needed() {
    if [ -f "$LOG_FILE" ]; then
        LOG_SIZE="$(wc -c < "$LOG_FILE" 2>/dev/null || echo 0)"
        if [ "${LOG_SIZE:-0}" -ge "$MAX_LOG_BYTES" ]; then
            mv -f "$LOG_FILE" "$LOG_FILE.1" 2>/dev/null || : > "$LOG_FILE"
        fi
    fi
}

BACKOFF_SECS=1

while true; do
    rotate_log_if_needed

    if [ -f "$STOP_SENTINEL" ]; then
        BACKOFF_SECS=1
        sleep 3
        continue
    fi

    if [ ! -x "$BIN_PATH" ]; then
        sleep 10
        continue
    fi

    START_TS="$(date +%s 2>/dev/null || echo 0)"
    "$BIN_PATH" --config "$CONFIG_PATH" >>"$LOG_FILE" 2>&1 &
    CHILD_PID=$!
    echo "$CHILD_PID" > "$PID_FILE"

    if [ -w "/proc/$CHILD_PID/oom_score_adj" ]; then
        echo -1000 > "/proc/$CHILD_PID/oom_score_adj" 2>/dev/null || true
    fi

    wait "$CHILD_PID"
    EXIT_CODE=$?
    rm -f "$PID_FILE"

    if [ -f "$STOP_SENTINEL" ]; then
        BACKOFF_SECS=1
        sleep 3
        continue
    fi

    END_TS="$(date +%s 2>/dev/null || echo 0)"
    RUNTIME=$((END_TS - START_TS))

    if [ "$EXIT_CODE" -eq 0 ] || [ "$RUNTIME" -ge 5 ]; then
        BACKOFF_SECS=1
    else
        BACKOFF_SECS=$((BACKOFF_SECS * 2))
        if [ "$BACKOFF_SECS" -gt 30 ]; then
            BACKOFF_SECS=30
        fi
    fi

    sleep "$BACKOFF_SECS"
done
