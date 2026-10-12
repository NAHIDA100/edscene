#!/usr/bin/env bash
# 在任意 amd64 Ubuntu/Debian 上把 edscene 编成 Linux 产物：ELF + .deb + .tar.gz。
#
#   ./packaging/linux/build-linux.sh              # 完整构建
#   ./packaging/linux/build-linux.sh --check      # 只跑测试与类型检查
#   ./packaging/linux/build-linux.sh --no-deb     # 跳过 dpkg-deb
#
# 脚本自己装缺的东西（系统依赖 + rustup），但只在"当前缺"时才动手：
# 已经配好的机器上重跑一次不会覆盖你的 cargo 配置或换掉你的工具链。
# 网络在国内时走 rsproxy 镜像，由 RUSTUP_DIST_SERVER / CARGO_MIRROR 控制。
set -euo pipefail

VERSION="${VERSION:-0.1.0}"
CHECK_ONLY=0
MAKE_DEB=1
MIRROR="${CARGO_MIRROR:-https://rsproxy.cn/index/}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

for a in "$@"; do
  case "$a" in
    --check) CHECK_ONLY=1 ;;
    --no-deb) MAKE_DEB=0 ;;
    *) echo "未知参数 $a（可用：--check / --no-deb）" >&2; exit 2 ;;
  esac
done

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

have() { command -v "$1" >/dev/null 2>&1; }

need_apt=()
# ALSA/PipeWire 输出、D-Bus、字体与输入：Slint 与 cpal 在 Linux 上要它们的头文件。
for pkg in build-essential pkg-config libasound2-dev libdbus-1-dev \
           libfontconfig1-dev libxkbcommon-dev dpkg; do
  if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q 'install ok installed'; then
    need_apt+=("$pkg")
  fi
done

if ((${#need_apt[@]})); then
  say "安装系统依赖：${need_apt[*]}"
  sudo apt-get update -qq
  sudo apt-get install -y --no-install-recommends "${need_apt[@]}"
fi

if ! have cargo; then
  say "安装 Rust（rustup，stable）"
  export RUSTUP_DIST_SERVER="${RUSTUP_DIST_SERVER:-https://rsproxy.cn}"
  export RUSTUP_UPDATE_ROOT="${RUSTUP_UPDATE_ROOT:-https://rsproxy.cn/rustup}"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable
fi
export PATH="$HOME/.cargo/bin:$PATH"
cargo --version
rustc --version

# 直连 static.crates.io 在国内会被 TLS 重置：没有镜像配置就补一条，有就不动。
if [[ ! -f "$HOME/.cargo/config.toml" && ! -f "$HOME/.cargo/config" ]]; then
  say "写入 crates.io 镜像（$MIRROR）"
  mkdir -p "$HOME/.cargo"
  cat > "$HOME/.cargo/config.toml" <<EOF
[source.crates-io]
replace-with = "rsproxy"

[source.rsproxy]
registry = "sparse+$MIRROR"

[registries.rsproxy]
index = "$MIRROR"

[net]
git-fetch-with-cli = true
EOF
fi

say "测试"
cargo test --workspace

if ((CHECK_ONLY)); then
  say "只检查模式，结束"
  exit 0
fi

say "发布构建"
cargo build --release --locked -p eds-app
BIN="target/release/edscene"
file "$BIN"
"$BIN" --config-path || true

say "打包 tar.gz（附带的 .desktop 与 systemd 单元来自 packaging/）"
STAGE="$(mktemp -d)"
install -m755 "$BIN" "$STAGE/edscene"
cp packaging/systemd/edscene-system.service "$STAGE/" 2>/dev/null || true
tar -C "$STAGE" -czf "edscene-$VERSION-linux-x64.tar.gz" .
rm -rf "$STAGE"

if ((MAKE_DEB)); then
  say "打包 deb"
  # bash 而不是 ./：从 Windows 上解出来的压缩包不一定带执行位。
  VERSIONED="$VERSION" bash packaging/build-deb.sh "$BIN" "edscene-$VERSION-linux-x64.deb"
fi

say "产物与校验和"
ls -l "edscene-$VERSION-linux-x64.tar.gz" edscene_*.deb 2>/dev/null || true
sha256sum "edscene-$VERSION-linux-x64.tar.gz" "$BIN" edscene_*.deb 2>/dev/null || true
