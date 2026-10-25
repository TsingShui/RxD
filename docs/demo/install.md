# Install or update RxD

The POSIX shell bootstrap installs a prebuilt RxD executable. It becomes the
supported entry point once a complete, verified stable Release is published. A
local smoke is not proof that anonymous installation works. The command below
resolves the latest stable release:

```sh
curl -fsSL https://github.com/TsingShui/RxD/releases/latest/download/install.sh | sh
```

Run that same command to actively update to the latest stable version. Latest is
resolved once via GitHub's stable Release redirect, then archive and checksum
requests use that exact tag. Drafts/prereleases and Actions artifacts are never
selected. A resolution/HTTP/rate-limit failure fails without choosing another
version. There is no API token, jq, Python or GitHub CLI runtime requirement.

After public publication, choose an exact version and directory using:

```sh
curl -fsSL https://github.com/TsingShui/RxD/releases/latest/download/install.sh | sh -s -- --version v0.1.0 --dir "$HOME/tools/rxd bin"
```

To inspect before executing, download both files from one concrete tag, verify the
checksum, read the script, then execute it (macOS: `shasum -a 256 -c`):

```sh
curl -fsSL https://github.com/TsingShui/RxD/releases/download/v0.1.0/install.sh -o install.sh
curl -fsSL https://github.com/TsingShui/RxD/releases/download/v0.1.0/install.sh.sha256 -o install.sh.sha256
sha256sum -c install.sh.sha256
cat install.sh
sh install.sh --version v0.1.0 --dir "$HOME/.local/bin"
```

These URLs become usable only after that version is published. With a local copy
of the script and accessible official Release assets, its interface is:

```sh
sh install.sh --help
sh install.sh                       # latest stable
sh install.sh --version v0.1.0     # exact tag, does not resolve latest
sh install.sh --version v0.1.0 --dir "$HOME/tools/rxd bin"
```

The default directory is `$HOME/.local/bin`. Run the same command again to replace
the installed executable with the latest stable or explicitly selected version. Use another exact
`vMAJOR.MINOR.PATCH` tag to upgrade or intentionally install an older version.
Installation verifies the archive's sibling SHA-256 file and the executable's
version, then replaces the executable atomically. A failed download, validation,
staging or replacement preserves the previous executable. No background update
service is installed. Running Agent/MCP processes must reload to use a new binary.

Supported platforms are Linux x86_64, Linux arm64 and macOS arm64. Linux requires
glibc 2.39 or newer; musl/Alpine, older glibc and other platforms are unsupported.
The shell entry point does not install Windows distributions. Required tools are
POSIX shell utilities, curl, tar, mktemp and either sha256sum or shasum. No Rust,
Python, Node, GitHub CLI or .NET SDK/runtime is required by the installer.

The installer reports the absolute executable path and a quoted version command
when the directory is outside PATH. It never invokes sudo or edits shell startup
files, Agent configuration or skills. A directory can contain spaces or Unicode.
Downloads and staging files are private and cleaned on normal exits and catchable
HUP/INT/TERM signals. SIGKILL and power failure cannot run shell cleanup traps;
atomic replacement still avoids a partially overwritten executable.

## Offline verification

```sh
python3 scripts/test-install.py
RXD_INSTALL_TEST_BINARY=/absolute/path/to/native/rxd python3 scripts/test-install.py
```

The first command uses explicitly authored executable fixtures to test bootstrap
boundaries. The second also packages a real native executable, serves it over
loopback HTTP and installs it twice through an actual curl-to-shell pipeline.
Platform mapping fixtures do not establish native execution on other platforms.
CI runs the real executable smoke on each supported Unix runner after packaging.
The Release job additionally executes the public versioned script and latest
update path after publication. Those remote checks remain unexecuted until live
publication is authorized; CI configuration alone does not establish a pass.

`RXD_INSTALL_TEST_BASE_URL` is reserved for deterministic offline tests and only
accepts an explicit `http://127.0.0.1:PORT` endpoint. Without that override,
archive and checksum downloads use the project's official HTTPS Release source,
including HTTPS-only redirects. It is not a supported custom-mirror interface.

## PATH and troubleshooting

The installed executable is normally `$HOME/.local/bin/rxd`. Use its absolute path
immediately, or set PATH in the current shell with `export PATH="$HOME/.local/bin:$PATH"`.
For a custom directory containing spaces, quote the entire executable path.
No shell startup file is edited. The installer prints a usable quoted version
command if the directory is outside PATH.

An HTTP error can mean no stable Release exists, the version/assets are absent,
or the release is not public. Check public availability and retry explicitly;
the installer does not silently switch sources. A SHA-256 or version mismatch
fails before replacement. Runtime errors require a supported native platform;
Linux requires glibc 2.39 or newer. Use a user-writable directory with `--dir`
if destination creation or replacement fails. Existing bytes are preserved on
failed updates, including latest resolution, download and checksum failures.

## Agent onboarding

Install the binary first; the bootstrap never edits Agent configuration. With an
MCP-enabled RxD binary, run the stdio server from an intended analysis directory
(not HOME or a filesystem root), and register that exact command in the client's
own user configuration:

```sh
cd /absolute/path/to/inputs
rxd mcp --base /absolute/path/to/inputs --root /absolute/path/to/inputs
```

RxD writes no client configuration. Register the absolute executable and argv
array yourself; Pi/CC use JSON `mcpServers.rxd`, Codex uses TOML
`mcp_servers.rxd`:

```json
{"mcpServers":{"rxd":{"command":"/absolute/path/rxd","args":["mcp","--base","/absolute/path/to/inputs","--root","/absolute/path/to/inputs"]}}}
```

```toml
[mcp_servers.rxd]
command = "/absolute/path/rxd"
args = ["mcp", "--base", "/absolute/path/to/inputs", "--root", "/absolute/path/to/inputs"]
```

Use an argv array, never a shell string. `--root` limits input capture, not an OS
sandbox. Backend names follow CLI namespaces (`jar`, not `jvm`) and must exist in
the build. Service ceilings and stdio behavior are listed by `rxd mcp --help` and
[the MCP demo](mcp.md).

### Reload and verify in the actual client

A written configuration is **not** a connected session. Startup controls, project
precedence, profiles and managed policy can override it; RxD does not change trust,
approval, policy, enablement or credentials to force a connection.

| Client | Reload and native verification |
| --- | --- |
| Pi | `/reload`, then `/mcp` in the session. Separately, `pi mcp list --json` connects/discovers from shell and reports errors. |
| CC | Restart the session; inspect `/mcp`. `claude mcp get rxd` and `claude mcp list` must report **Connected**, not merely exit zero. |
| Codex | Restart the session; inspect `/mcp`. `codex mcp get rxd --json` and `codex mcp list --json` prove native readback, not a tool call. App-server integrations can use `config/mcpServer/reload` when supported. |

Pi support uses native MCP, not a third-party bridge. The conservative verification
floor here is **Pi 1.3.0** (actual SDK session route); prior Pi 1.2.0 native CLI
help was observed during spec preparation, not revalidated as a session here.
This is **not** a claim that every older version is incompatible. Tested CC is
**2.1.301**, Codex is **0.162.1**; earlier versions are not attested by these
native checks.

Pi's default MCP exposure is codemode, so a connected tool need not be in the
active/direct tool set. Trusted `.pi/mcp.json` can replace same-name user entries;
a project entry with only enabled/exposure/toolExposure overrides just those
controls. `enabled: false`, hidden exposure, tool filters, `--no-mcp`,
`--no-extensions`, or `extensions: ["-builtin:mcp"]` can prevent session use.
An extension registering `/mcp` replaces built-in session MCP: shell `pi mcp list`
can still connect while that session ignores native `mcp.json`. Inspect your
session's extensions and settings and choose any changes yourself; RxD does not
remove a bridge or enable a built-in.

CC local/project entries override user entries; managed allow/deny policies and
per-project disabled servers still apply. Codex CLI overrides, trusted project
layers, selected profiles (0.162.1 supports `$CODEX_HOME/<profile>.config.toml`)
and managed requirements may supersede an explicit config. Never grant trust or
weaken policy merely to make a probe pass.

## Skills and unsupported-client fallback

`rxd skill` prints the embedded generic skill offline and writes nothing:

```sh
rxd skill
```

The skill covers all current backends. Placing the Markdown where an Agent discovers
it is the caller's job; `rxd skill` creates no directories or Agent configuration.
See [README](../../README.md#-installation). The old `rxd dex skill` entry is removed.
Unsupported clients can still use manual stdio registration with the exact absolute
executable/base/roots: see the [MCP demo](mcp.md). Never put a shell command string in
an argv field.
