#!/bin/sh
set -eu

APP_BIN="/usr/local/bin/smelly-connect-cli"
APP_USER="vpn"
APP_GROUP="vpn"
RUNTIME_CONFIG_DIR="/run/smelly-connect"
RUNTIME_CONFIG_PATH="$RUNTIME_CONFIG_DIR/config.toml"

config_src=""
prev_was_config=0

for arg in "$@"; do
    if [ "$prev_was_config" -eq 1 ]; then
        config_src="$arg"
        break
    fi
    if [ "$arg" = "--config" ]; then
        prev_was_config=1
    fi
done

if [ -n "$config_src" ]; then
    install -d -o "$APP_USER" -g "$APP_GROUP" "$RUNTIME_CONFIG_DIR"
    install -m 600 -o "$APP_USER" -g "$APP_GROUP" "$config_src" "$RUNTIME_CONFIG_PATH"

    rewritten_args=""
    prev_was_config=0
    for arg in "$@"; do
        if [ "$prev_was_config" -eq 1 ]; then
            rewritten_args="$rewritten_args '$RUNTIME_CONFIG_PATH'"
            prev_was_config=0
            continue
        fi
        if [ "$arg" = "--config" ]; then
            prev_was_config=1
        fi
        rewritten_args="$rewritten_args '$(printf "%s" "$arg" | sed "s/'/'\\\\''/g")'"
    done
    eval "set -- $rewritten_args"
fi

quoted_args=""
for arg in "$@"; do
    quoted_args="$quoted_args '$(printf "%s" "$arg" | sed "s/'/'\\\\''/g")'"
done

exec su -s /bin/sh "$APP_USER" -c "exec \"$APP_BIN\"$quoted_args"
