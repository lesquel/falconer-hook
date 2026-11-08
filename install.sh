#!/bin/sh
# openmosh-hook installer.
#
#   curl -fsSL https://raw.githubusercontent.com/lesquel/openmosh-hook/main/install.sh | sh
#
# Installs openmosh-hook to ~/.local/bin, checks sshd, Herdr/tmux and
# Tailscale, writes a default config, registers the user service and runs
# `openmosh-hook doctor`. It prints every command before running it and
# never uses sudo itself; only the official Herdr/Tailscale installers it
# offers may ask for root.
set -eu

REPO="lesquel/openmosh-hook"
BIN_DIR="${OPENMOSH_BIN_DIR:-$HOME/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/openmosh"
DRY_RUN=0
MODE=release
BINARY=""
VERSION="latest"
ASSUME_YES=0
NO_SERVICE=0

usage() {
	cat <<'EOF'
Usage: install.sh [options]

  --dry-run          Print what would be done; change nothing.
  --from-source      Build with `cargo install --git` (needs Rust and, while the
                     repository is private, git access to it).
  --binary <path>    Install an already-built openmosh-hook binary.
  --version <tag>    Release tag to download (default: latest).
  --yes              Accept offers to install missing Herdr/Tailscale.
  --no-service       Do not run `openmosh-hook service install`.
  -h, --help         Show this help.

Environment: OPENMOSH_BIN_DIR (default ~/.local/bin).
EOF
}

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}

# Prints a command, then runs it unless --dry-run.
run() {
	printf '+ %s\n' "$*"
	if [ "$DRY_RUN" -eq 0 ]; then
		"$@"
	fi
}

have() { command -v "$1" >/dev/null 2>&1; }

# Asks a yes/no question on the terminal (works under `curl | sh`).
confirm() {
	if [ "$DRY_RUN" -eq 1 ]; then
		say "(dry run: would ask) $1 [y/N]"
		return 1
	fi
	if [ "$ASSUME_YES" -eq 1 ]; then
		return 0
	fi
	if [ ! -r /dev/tty ]; then
		say "$1 [y/N] -> no terminal, skipping (use --yes to accept)"
		return 1
	fi
	printf '%s [y/N] ' "$1"
	read -r answer </dev/tty || answer=""
	case "$answer" in
	y | Y | yes | YES) return 0 ;;
	*) return 1 ;;
	esac
}

while [ $# -gt 0 ]; do
	case "$1" in
	--dry-run) DRY_RUN=1 ;;
	--from-source) MODE=source ;;
	--binary)
		[ $# -ge 2 ] || die "--binary needs a path"
		MODE=binary
		BINARY="$2"
		shift
		;;
	--version)
		[ $# -ge 2 ] || die "--version needs a tag"
		VERSION="$2"
		shift
		;;
	--yes) ASSUME_YES=1 ;;
	--no-service) NO_SERVICE=1 ;;
	-h | --help)
		usage
		exit 0
		;;
	*) die "unknown option: $1 (see --help)" ;;
	esac
	shift
done

[ -n "${HOME:-}" ] || die "HOME is not set"
if [ "$(id -u)" -eq 0 ]; then
	die "run this installer as your normal user, not root"
fi

detect_target() {
	os=$(uname -s)
	arch=$(uname -m)
	case "$arch" in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	*) die "unsupported architecture: $arch" ;;
	esac
	case "$os" in
	Linux) TARGET="$arch-unknown-linux-musl" ;;
	Darwin) TARGET="universal-apple-darwin" ;;
	*) die "unsupported OS: $os (Linux and macOS are supported)" ;;
	esac
}

download() {
	if have curl; then
		run curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
	elif have wget; then
		run wget -q --https-only -O "$2" "$1"
	else
		die "curl or wget is required"
	fi
}

verify_sha256() {
	expected=$(cut -d ' ' -f 1 <"$2")
	if have sha256sum; then
		actual=$(sha256sum "$1" | cut -d ' ' -f 1)
	elif have shasum; then
		actual=$(shasum -a 256 "$1" | cut -d ' ' -f 1)
	else
		die "sha256sum or shasum is required to verify the download"
	fi
	[ "$expected" = "$actual" ] || die "checksum mismatch for $1"
}

install_release() {
	detect_target
	asset="openmosh-hook-$TARGET.tar.gz"
	if [ "$VERSION" = latest ]; then
		base="https://github.com/$REPO/releases/latest/download"
	else
		base="https://github.com/$REPO/releases/download/$VERSION"
	fi
	say "Installing openmosh-hook ($TARGET) from GitHub Releases"
	if [ "$DRY_RUN" -eq 1 ]; then
		tmp="<tmpdir>"
	else
		tmp=$(mktemp -d)
		trap 'rm -rf "$tmp"' EXIT
	fi
	if ! download "$base/$asset" "$tmp/$asset" || ! download "$base/$asset.sha256" "$tmp/$asset.sha256"; then
		die "download failed. If no release is published yet (or the repository is private), use --from-source or --binary <path>"
	fi
	if [ "$DRY_RUN" -eq 0 ]; then
		verify_sha256 "$tmp/$asset" "$tmp/$asset.sha256"
	fi
	run mkdir -p "$BIN_DIR"
	run tar -xzf "$tmp/$asset" -C "$tmp"
	run install -m 0755 "$tmp/openmosh-hook" "$BIN_DIR/openmosh-hook"
}

install_source() {
	have cargo || die "cargo not found; install Rust from https://rustup.rs first"
	say "Building openmosh-hook from source (the repository is private: your git credentials are used)"
	# cargo installs into <root>/bin, i.e. ~/.local/bin by default.
	root=$(dirname "$BIN_DIR")
	CARGO_NET_GIT_FETCH_WITH_CLI=true
	export CARGO_NET_GIT_FETCH_WITH_CLI
	run cargo install --git "https://github.com/$REPO" --locked --root "$root" openmosh-hook
}

install_binary() {
	[ -f "$BINARY" ] || die "binary not found: $BINARY"
	run mkdir -p "$BIN_DIR"
	run install -m 0755 "$BINARY" "$BIN_DIR/openmosh-hook"
}

check_sshd() {
	if have sshd || [ -x /usr/sbin/sshd ] || [ -x /usr/bin/sshd ]; then
		say "ok: sshd found"
	else
		warn "sshd not found. Install and enable the OpenSSH server with your package manager"
		warn "(e.g. 'sudo apt install openssh-server' or 'sudo pacman -S openssh && sudo systemctl enable --now sshd')."
	fi
}

check_runtime() {
	if have herdr; then
		say "ok: herdr found"
	elif have tmux; then
		say "ok: tmux found (Herdr is recommended for automatic agent detection)"
	else
		warn "neither Herdr nor tmux is installed"
		if confirm "Install Herdr with its official installer (curl -fsSL https://herdr.dev/install.sh | sh)?"; then
			say "+ curl -fsSL https://herdr.dev/install.sh | sh"
			curl -fsSL https://herdr.dev/install.sh | sh
		else
			say "Install Herdr (https://herdr.dev) or tmux, then run: openmosh-hook doctor"
		fi
	fi
}

check_tailscale() {
	if have tailscale; then
		if tailscale status --json 2>/dev/null | grep -q '"BackendState": *"Running"'; then
			say "ok: tailscale running"
		else
			warn "tailscale is installed but not running. Run: sudo tailscale up"
		fi
	else
		warn "tailscale is not installed (required for pairing)"
		if confirm "Install Tailscale with its official installer (curl -fsSL https://tailscale.com/install.sh | sh)? It asks for sudo itself."; then
			say "+ curl -fsSL https://tailscale.com/install.sh | sh"
			curl -fsSL https://tailscale.com/install.sh | sh
			say "Now run: sudo tailscale up"
		else
			say "Install Tailscale from https://tailscale.com/download, then run: sudo tailscale up"
		fi
	fi
}

write_config() {
	file="$CONFIG_DIR/config.toml"
	if [ -e "$file" ]; then
		say "ok: keeping existing $file"
		return
	fi
	say "Writing $file (mode 0600)"
	if [ "$DRY_RUN" -eq 1 ]; then
		say "+ mkdir -p $CONFIG_DIR && chmod 700 $CONFIG_DIR"
		say "+ write $file && chmod 600 $file"
		return
	fi
	(
		umask 077
		mkdir -p "$CONFIG_DIR"
		chmod 700 "$CONFIG_DIR"
		cat >"$file" <<'EOF'
# openmosh-hook configuration. Keep this file mode 0600.
# host_alias = "my-laptop"   # name shown on the phone (default: hostname)
retention_days = 7
poll_interval_ms = 1000
sources = ["herdr", "tmux"]
ssh_port = 22

[push]
# Push transport suggested to the phone. ntfy.sh only sees ciphertext;
# a self-hosted ntfy inside your tailnet also works.
server = "https://ntfy.sh"
EOF
		chmod 600 "$file"
	)
}

if [ "$DRY_RUN" -eq 1 ]; then
	say "openmosh-hook installer (dry run: nothing will be changed)"
else
	say "openmosh-hook installer"
fi
case "$MODE" in
release) install_release ;;
source) install_source ;;
binary) install_binary ;;
esac

check_sshd
check_runtime
check_tailscale
write_config

HOOK="$BIN_DIR/openmosh-hook"
if [ "$NO_SERVICE" -eq 0 ]; then
	run "$HOOK" service install
fi
if [ "$DRY_RUN" -eq 0 ]; then
	say "+ $HOOK doctor"
	"$HOOK" doctor || true
else
	say "+ $HOOK doctor"
fi

case ":$PATH:" in
*":$BIN_DIR:"*) ;;
*) say "Note: add $BIN_DIR to your PATH." ;;
esac
say ""
say "Next: open OpenMosh on your phone and add this host, then run: openmosh-hook pair"
