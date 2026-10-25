#!/bin/sh
# RxD release bootstrap. No source, Agent or shell configuration is executed/edited.
set -eu
umask 077
fail() { printf 'rxd install: %s\n' "$*" >&2; exit 1; }
usage() {
    printf '%s\n' 'Usage: sh install.sh [--version vMAJOR.MINOR.PATCH] [--dir DIRECTORY]' \
        'Install or update RxD from the latest stable Release, or an exact tag (directory: ~/.local/bin).'
}
tag=
install_dir=${HOME:+$HOME/.local/bin}
while [ "$#" -gt 0 ]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --version|--dir)
            option=$1; shift
            [ "$#" -gt 0 ] && [ -n "$1" ] || fail "$option requires a value"
            case "$option" in --version) tag=$1 ;; --dir) install_dir=$1 ;; esac ;;
        *) fail "unknown argument: $1" ;;
    esac
    shift
done
[ -n "$install_dir" ] || fail 'HOME is unset; specify --dir'
for tool in curl tar mktemp uname mkdir chmod mv rm cat cmp tr grep sed; do
    command -v "$tool" >/dev/null 2>&1 || fail "missing required tool: $tool"
done
validate_tag() {
    printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail 'invalid version tag; expected vMAJOR.MINOR.PATCH'
}
[ -z "$tag" ] || validate_tag
if command -v sha256sum >/dev/null 2>&1; then
    hash_tool=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    hash_tool=shasum
else
    fail 'missing SHA-256 tool: install sha256sum or shasum'
fi
system=$(uname -s)
machine=$(uname -m)
case "$machine" in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=x86_64 ;; *) fail "unsupported architecture: $machine" ;; esac
case "$system-$arch" in
    Darwin-arm64) platform=macos-arm64 ;;
    Linux-arm64|Linux-x86_64)
        command -v getconf >/dev/null 2>&1 || fail 'Linux requires glibc 2.39 or newer (getconf unavailable)'
        libc=$(getconf GNU_LIBC_VERSION 2>/dev/null) || fail 'Linux requires glibc 2.39 or newer; musl is unsupported'
        case "$libc" in 'glibc '*) libc=${libc#glibc } ;; *) fail 'Linux requires glibc 2.39 or newer; musl is unsupported' ;; esac
        major=${libc%%.*}; minor=${libc#*.}; minor=${minor%%.*}
        case "$major:$minor" in *[!0-9:]*|:*|*:) fail 'cannot determine glibc version' ;; esac
        if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -lt 39 ]; }; then
            fail "glibc $libc is unsupported; glibc 2.39 or newer is required"
        fi
        platform=linux-$arch ;;
    *) fail "unsupported platform: $system $machine" ;;
esac
base_url=https://github.com/TsingShui/RxD/releases
protocols='=https'
# Offline E2E boundary only. Never accept arbitrary HTTP mirrors or redirects.
if [ -n "${RXD_INSTALL_TEST_BASE_URL:-}" ]; then
    case "$RXD_INSTALL_TEST_BASE_URL" in
        http://127.0.0.1:[0-9]*)
            port=${RXD_INSTALL_TEST_BASE_URL#http://127.0.0.1:}
            case "$port" in ''|*[!0-9]*) fail 'invalid loopback test port' ;; esac ;;
        *) fail 'test source must be an explicit IPv4 loopback HTTP port' ;;
    esac
    base_url=$RXD_INSTALL_TEST_BASE_URL
    protocols='=http'
fi
work=
stage=
child=
cleanup() {
    [ -z "$child" ] || kill "$child" 2>/dev/null || :
    [ -z "$work" ] || rm -rf -- "$work"
    [ -z "$stage" ] || rm -rf -- "$stage"
    :
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
work=$(mktemp -d "${TMPDIR:-/tmp}/rxd-install.XXXXXXXX") || fail 'cannot create temporary directory'
archive=rxd-$platform.tar.gz
fetch() {
    curl -q --proto "$protocols" --proto-redir "$protocols" --fail --silent --show-error --location \
        --connect-timeout 30 --max-time 600 "$1" --output "$2" --write-out "${3:-}" || fail "download failed: $1"
}
if [ -z "$tag" ]; then
    # GitHub's stable-only redirect selects one completed, non-prerelease tag.
    # Parse the final canonical URL, not HTML or JSON requiring an extra runtime.
    resolved=$(fetch "$base_url/latest" "$work/latest" '%{url_effective}')
    case "$resolved" in
        "$base_url/tag/"*) tag=${resolved#"$base_url/tag/"} ;;
        *) fail "latest stable Release did not resolve to an official tag: $resolved" ;;
    esac
    validate_tag
fi
fetch "$base_url/download/$tag/$archive" "$work/$archive"
fetch "$base_url/download/$tag/$archive.sha256" "$work/checksum"
line=$(cat "$work/checksum")
digest=${line%% *}
[ "${#digest}" -eq 64 ] || fail 'malformed SHA-256 file'
case "$digest" in *[!0-9a-fA-F]*) fail 'malformed SHA-256 digest' ;; esac
# Require one checksum, with the exact archive name and a terminating newline.
printf '%s  %s\n' "$digest" "$archive" | cmp -s - "$work/checksum" || fail 'malformed SHA-256 file or wrong archive name'
if [ "$hash_tool" = sha256sum ]; then
    actual=$(sha256sum "$work/$archive")
else
    actual=$(shasum -a 256 "$work/$archive")
fi
actual=${actual%% *}
expected=$(printf '%s' "$digest" | tr A-F a-f)
[ "$actual" = "$expected" ] || fail 'SHA-256 mismatch'
entry=rxd-$platform/rxd
# Validate one regular archive member; stream only its bytes without extracting paths.
tar -tzf "$work/$archive" "$entry" > "$work/names" || fail 'invalid archive or missing executable'
printf '%s\n' "$entry" | cmp -s - "$work/names" || fail 'archive must contain exactly one expected executable'
tar -tvzf "$work/$archive" "$entry" > "$work/types" || fail 'cannot inspect executable type'
IFS= read -r type < "$work/types" || fail 'missing executable type'
case "$type" in -*) ;; *) fail 'archive executable must be a regular file' ;; esac
# Normalize relative paths before external tools interpret a leading dash.
case "$install_dir" in /*) ;; *) install_dir=$PWD/$install_dir ;; esac
mkdir -p -- "$install_dir" || fail "cannot create destination: $install_dir"
install_dir=$(CDPATH= cd -- "$install_dir" && pwd -P) || fail 'cannot resolve destination'
[ ! -d "$install_dir/rxd" ] || fail 'destination rxd is a directory'
stage=$(mktemp -d "$install_dir/.rxd-install.XXXXXXXX") || fail 'cannot stage on destination filesystem'
tar -xzOf "$work/$archive" "$entry" > "$stage/rxd" || fail 'cannot extract executable'
chmod 755 "$stage/rxd" || fail 'cannot set executable permissions'
# wait is interruptible by shell traps; a synchronous command substitution may
# defer signals until after its child exits and allow an interrupted upgrade.
"$stage/rxd" --version > "$work/version" &
child=$!
if wait "$child"; then
    child=
else
    child=
    fail 'staged executable failed --version (check platform/runtime compatibility)'
fi
version=$(cat "$work/version")
[ "$version" = "rxd ${tag#v}" ] || fail "release version mismatch: expected rxd ${tag#v}, got $version"
mv -f -- "$stage/rxd" "$install_dir/rxd" || fail 'cannot replace executable'
printf 'Installed %s at %s\n' "$version" "$install_dir/rxd"
case ":${PATH:-}:" in
    *":$install_dir:"*) ;;
    *) printf 'Use the installed executable: '; printf "'"; printf '%s' "$install_dir/rxd" | sed "s/'/'\\\\''/g"; printf "' --version\n" ;;
esac
