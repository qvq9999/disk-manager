#!/usr/bin/env bash
# 通用开发环境模板 —— 复制为 dev-env.sh 后按自己的路径修改
#
#   cp dev-env.example.sh dev-env.sh
#   source ./dev-env.sh
#
# 大多数机器上 rustup / VS Build Tools 装在默认位置，rustc 能自动探测到链接器，
# 这时**不需要**本文件的任何路径配置，直接 `npx tauri build` 即可。
# 只有在 rustc 报 `link.exe not found` 或误用 GNU link 时才需要手动指定。

# 如 rustup 不在默认位置，取消注释并改成自己的路径：
# export RUSTUP_HOME="$HOME/.rustup"
# export CARGO_HOME="$HOME/.cargo"

# 仅当自动探测失败时，才显式指定 MSVC 工具链（把路径换成自己的）：
# export MSVC_DIR='/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207'
# export WINSDK_DIR='/c/Program Files (x86)/Windows Kits/10'
# export PATH="$(cygpath -u "$MSVC_DIR")/bin/Hostx64/x64:$WINSDK_DIR/bin/10.0.26100.0/x64:$PATH"
# export INCLUDE="$MSVC_DIR\\include;$WINSDK_DIR\\Include\\10.0.26100.0\\um;$WINSDK_DIR\\Include\\10.0.26100.0\\ucrt;$WINSDK_DIR\\Include\\10.0.26100.0\\shared"
# export LIB="$MSVC_DIR\\lib\\x64;$WINSDK_DIR\\Lib\\10.0.26100.0\\um\\x64;$WINSDK_DIR\\Lib\\10.0.26100.0\\ucrt\\x64"

echo "[dev-env] cargo=$(cargo --version 2>/dev/null) | rustc=$(rustc --version 2>/dev/null)"
