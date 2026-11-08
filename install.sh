#!/bin/sh
# openmosh-hook installer.
#
#   curl -fsSL https://raw.githubusercontent.com/lesquel/openmosh-hook/main/install.sh | sh
#
# While the repository is private, anonymous downloads fail: install and
# log in to the GitHub CLI (`gh auth login`) and the installer downloads the
# release with `gh release download` instead (or run it from a clone).
#
# Installs openmosh-hook to ~/.local/bin, checks sshd, Herdr/tmux and
# Tailscale, writes a default config, registers the user service and runs
# `openmosh-hook doctor`. It prints every command before running it and
# never uses sudo itself; only the official Herdr/Tailscale installers it
# offers may ask for root.
set -eu

REPO="lesquel/openmosh-hook"
# Release signing key (minisign, key id 59E37F1EF15B67E2); also in minisign.pub.
PUBKEY="RWTiZ1vxHn/jWfqAEjobeqloI9cL9pOA0/in+ayyCgPqT0gYDRKR1Ox5"
BIN_DIR="${OPENMOSH_BIN_DIR:-$HOME/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/openmosh"
DRY_RUN=0
MODE=release
BINARY=""
VERSION="latest"
ASSUME_YES=0
NO_SERVICE=0
REQUIRE_SIG=0

usage() {
	cat <<'EOF'
Usage: install.sh [options]

  --dry-run          Print what would be done; change nothing.
  --from-source      Build with `cargo install --git` (needs Rust and, while the
                     repository is private, git access to it).
  --binary <path>    Install an already-built openmosh-hook binary.
  --version <tag>    Release tag to download (default: latest).
                     Downloads use `gh release download` when the GitHub CLI
                     is installed and logged in (needed while the repository
                     is private), else curl/wget. Every download is checked
                     against the release's SHA256SUMS, and SHA256SUMS against
                     its minisign signature when minisign or rsign is installed.
  --require-signature
                     Abort unless the signature can be verified (needs
                     minisign or rsign).
  --yes              Accept offers to install missing Herdr/Tailscale.
  --no-service       Do not run `openmosh-hook service install`.
  -h, --help         Show this help.

Environment: OPENMOSH_BIN_DIR (default ~/.local/bin),
             OPENMOSH_NO_GH=1 (never use the GitHub CLI).
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
		# A tag goes into a download URL and a gh argument: allow release tags only.
		case "$VERSION" in
		v[0-9]*) ;;
		*) die "--version must be a release tag like v0.1.0" ;;
		esac
		case "$VERSION" in
		*[!0-9A-Za-z.+-]*) die "--version must be a release tag like v0.1.0" ;;
		esac
		shift
		;;
	--yes) ASSUME_YES=1 ;;
	--no-service) NO_SERVICE=1 ;;
	--require-signature) REQUIRE_SIG=1 ;;
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
	Darwin) TARGET="$arch-apple-darwin" ;;
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

# verify_sha256 <file> <SHA256SUMS>: the file's line in SHA256SUMS must match.
verify_sha256() {
	name=$(basename "$1")
	expected=$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1 }' "$2")
	[ -n "$expected" ] || die "$name is not listed in SHA256SUMS"
	if have sha256sum; then
		actual=$(sha256sum "$1" | cut -d ' ' -f 1)
	elif have shasum; then
		actual=$(shasum -a 256 "$1" | cut -d ' ' -f 1)
	else
		die "sha256sum or shasum is required to verify the download"
	fi
	[ "$expected" = "$actual" ] || die "checksum mismatch for $name"
}

# The minisign-compatible verifier on PATH, or empty.
sig_tool() {
	if have minisign; then
		echo minisign
	elif have rsign; then
		echo rsign
	fi
}

# verify_signature <SHA256SUMS> <SHA256SUMS.minisig>: checked with the pinned key.
verify_signature() {
	case "$(sig_tool)" in
	minisign) minisign -V -q -P "$PUBKEY" -m "$1" -x "$2" >/dev/null ||
		die "signature verification failed for SHA256SUMS" ;;
	rsign) rsign verify -q -P "$PUBKEY" -x "$2" "$1" >/dev/null ||
		die "signature verification failed for SHA256SUMS" ;;
	*) die "minisign or rsign is required to verify the signature" ;;
	esac
	say "ok: SHA256SUMS signature verified (key 59E37F1EF15B67E2)"
}

# Says how the download will be authenticated; aborts early when a required
# signature cannot be checked.
signature_policy() {
	tool=$(sig_tool)
	if [ -n "$tool" ]; then
		say "Signature: SHA256SUMS is verified with $tool and the pinned key 59E37F1EF15B67E2"
	elif [ "$REQUIRE_SIG" -eq 1 ]; then
		die "--require-signature: install minisign (https://jedisct1.github.io/minisign/) or rsign2 first"
	else
		warn "minisign/rsign not found: checking SHA256SUMS only, without its signature."
		warn "Install minisign to verify releases, or pass --require-signature to insist."
	fi
}

# The GitHub CLI is used when installed and logged in (private repository).
use_gh() {
	[ "${OPENMOSH_NO_GH:-0}" != 1 ] && have gh && gh auth status >/dev/null 2>&1
}

install_release() {
	detect_target
	asset="openmosh-hook-$TARGET.tar.gz"
	say "Installing openmosh-hook ($TARGET) from GitHub Releases ($VERSION)"
	signature_policy
	if [ "$DRY_RUN" -eq 1 ]; then
		tmp="<tmpdir>"
	else
		tmp=$(mktemp -d)
		trap 'rm -rf "$tmp"' EXIT
	fi
	if use_gh; then
		say "Using the GitHub CLI (logged in)"
		if [ "$VERSION" = latest ]; then
			tag=""
		else
			tag="$VERSION"
		fi
		# shellcheck disable=SC2086 # empty $tag means "latest release"
		run gh release download $tag --repo "$REPO" --pattern "$asset" --pattern SHA256SUMS --pattern SHA256SUMS.minisig --dir "$tmp" ||
			die "gh release download failed (is a release published for $TARGET?)"
	else
		if [ "$VERSION" = latest ]; then
			base="https://github.com/$REPO/releases/latest/download"
		else
			base="https://github.com/$REPO/releases/download/$VERSION"
		fi
		if ! download "$base/$asset" "$tmp/$asset" || ! download "$base/SHA256SUMS" "$tmp/SHA256SUMS"; then
			die "download failed. While the repository is private, run 'gh auth login' first (the installer then uses 'gh release download'), or use --from-source or --binary <path>"
		fi
		if [ -n "$(sig_tool)" ]; then
			download "$base/SHA256SUMS.minisig" "$tmp/SHA256SUMS.minisig" ||
				die "could not download SHA256SUMS.minisig"
		fi
	fi
	if [ "$DRY_RUN" -eq 0 ]; then
		if [ -n "$(sig_tool)" ]; then
			[ -f "$tmp/SHA256SUMS.minisig" ] || die "the release has no SHA256SUMS.minisig"
			verify_signature "$tmp/SHA256SUMS" "$tmp/SHA256SUMS.minisig"
		fi
		verify_sha256 "$tmp/$asset" "$tmp/SHA256SUMS"
	else
		if [ -n "$(sig_tool)" ]; then
			say "(dry run: would verify SHA256SUMS.minisig with $(sig_tool) and key $PUBKEY)"
		fi
		say "(dry run: would verify $asset against SHA256SUMS)"
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
say "Wire your AI agents' hooks (Claude Code, Codex, Gemini, Cursor, Copilot, Kiro, OpenCode, Aider):"
say "  $HOOK integration list && $HOOK setup"
say "Next: open OpenMosh on your phone and add this host, then run: openmosh-hook pair"
