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
MCP-enabled RxD binary, register the clients you use from an intended analysis
directory (not HOME or a filesystem root):

```sh
cd /absolute/path/to/inputs
rxd mcp install -a pi --preview
rxd mcp install -a pi
rxd mcp install -a cc -a codex
# All three targets, even if not detected:
rxd mcp install -a all
# Only detected supported clients; fails if none are detected:
rxd mcp install
rxd mcp install --help
```

Repeat `-a/--agent` for multiple clients. `claude` and `claude-code` alias `cc`;
duplicates are removed in pi/cc/codex order. There is no `agents` MCP target or
generic-directory fallback. Automatic detection means a
client executable on PATH **or** an existing resolved native config file/directory
(Pi agent dir, CC `~/.claude` or its override, Codex home). An override value alone
is not detection. Explicit selection can create the chosen user configuration.
No client is started, downloaded or upgraded; registration needs no network,
provider credential, separate server/runtime or engine initialization.

### Scope, paths and authorization

| Client | User configuration selected by RxD |
| --- | --- |
| Pi | `$PI_CODING_AGENT_DIR/mcp.json`, otherwise `~/.pi/agent/mcp.json` |
| CC / Claude Code | `$CLAUDE_CONFIG_DIR/.claude.json`, otherwise `~/.claude.json`; existing legacy `$CLAUDE_CONFIG_DIR/.config.json` or `~/.claude/.config.json` wins |
| Codex | `$CODEX_HOME/config.toml`, otherwise `~/.codex/config.toml` |

Empty overrides are unset; Pi's leading `~/` expands against HOME. These are
user-level destinations, not project/team/enterprise editors, Claude Desktop
settings, or skill directories. Invalid legacy CC config fails without writing a
different modern file. Pi/CC use JSON `mcpServers.rxd`; Codex uses TOML
`mcp_servers.rxd`. Unknown settings, other servers and credentials are retained;
TOML comments are retained. RxD never evaluates existing environment commands.

The stable name is `rxd`. Its command is the exact absolute current executable;
arguments are an array beginning `mcp`, `serve`, not shell text. Initially the
absolute base and allowed root are the installation working directory. The base
fixes relative query paths even when the client starts elsewhere. Explicit roots
limit input capture; this is not an OS sandbox. To choose a different base, several
roots or a catalog filter:

```sh
rxd mcp install -a pi -a claude-code -a codex \
  --base /absolute/path/to/inputs \
  --root /absolute/path/to/inputs --root /absolute/path/to/other-inputs \
  --backends js,jar,wasm --preview
# Remove --preview after inspecting the planned registration.
```

Roots/base must exist; relative roots resolve against base. Backend names follow
CLI namespaces (`jar`, not `jvm`) and must be present in the build. Without an
explicit root, defaults equal to/containing HOME or a filesystem/volume root are
refused, even with `--base` alone. Mounted roots are checked using native macOS
mount identity, bounded Linux namespace mount information (including bind mounts),
or Windows volume paths, not device-parent heuristics alone. If mount identity is
unavailable, implicit authority fails with guidance to choose explicit `--root`.
Select a narrow analysis directory; deliberate broad access requires explicit
`--root`, never an implicit default. Existing
recognized registrations retain roots/base/filter unless explicitly changed.
Resource ceilings and stdio behavior remain those of `mcp serve`.

### Preview, repeat, relocation and failure

`--preview` creates no files/directories. Stdout is one machine-consumable JSON
object with `preview` and ordered `targets` (agent, path, outcome, managed
registration, and Pi warning codes). It never echoes unrelated configuration or
credentials. Diagnostics/reload guidance go to stderr. Check the exit status and
**each** target: installed, updated, unchanged, conflict, failed, or not-written
(aborted preflight). Preview outcomes describe intended changes, not publication.

Identical repeats preserve exact bytes without rewriting. An in-place binary
update needs client restart, not registration changes. After relocation, run the
new absolute executable's `mcp install` to update the recognized entry. Recognition
requires valid stdio `mcp serve` argv and either the current executable path or the
exact conventional basename `rxd`/`rxd.exe`; `rxd.sh`, `rxd.py` and `rxd.wrapper`
are not recognized by that convention. It is not provenance proof. An old custom-named
executable after relocation is ambiguous, as is an unrelated `rxd` entry:

```sh
/absolute/new/location/rxd mcp install -a pi --preview
# Only after deciding to replace conflicting startup fields:
/absolute/new/location/rxd mcp install -a pi --replace
```

Replacement resets launch defaults unless supplied, but retains caller-owned entry
controls (`env`, enabled, exposure, timeouts) and unrelated servers. It does not
make a disabled entry active. Malformed/duplicate-key JSON, malformed TOML, wrong
shapes, unsafe files, invalid roots and known unwritable targets fail before
publication where detectable. Input and generated configurations must fit 4 MiB;
an oversized generated JSON/TOML document aborts all selected targets before any
staging or directory creation, including preview. Unrelated values are not truncated.
Files are individually staged/atomically replaced,
not a cross-client transaction: a late failure returns nonzero and reports actual
persisted outcomes; earlier successes are not rolled back. Concurrent snapshot
checks do not eliminate the unexercised final-check-to-rename write window. Do not
run concurrent configuration writers expecting compare-and-swap semantics.

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
This is **not** a claim that every older version is incompatible. If your Pi lacks
native `mcp` commands/support, check its version/help and separately choose a
supported client version; RxD never upgrades it. Tested CC is **2.1.301**, Codex is
**0.162.1**; earlier versions are not attested by these native checks.

Pi's default MCP exposure is codemode, so a connected tool need not be in the
active/direct tool set. Trusted `.pi/mcp.json` can replace same-name user entries;
a project entry with only enabled/exposure/toolExposure overrides just those
controls. `enabled: false`, hidden exposure, tool filters, `--no-mcp`,
`--no-extensions`, or `extensions: ["-builtin:mcp"]` can prevent session use.
An extension registering `/mcp` replaces built-in session MCP: shell `pi mcp list`
can still connect while that session ignores native `mcp.json`. RxD warns about
detectable disabled built-ins and extension-name hints only; arbitrary hooks
cannot be detected reliably. Inspect your session's extensions and settings and
choose any changes yourself; RxD does not remove a bridge or enable a built-in.

CC local/project entries override user entries; managed allow/deny policies and
per-project disabled servers still apply. Codex CLI overrides, trusted project
layers, selected profiles (0.162.1 supports `$CODEX_HOME/<profile>.config.toml`)
and managed requirements may supersede an explicit config. Never grant trust or
weaken policy merely to make a verification probe pass.

### Optional no-model acceptance tooling

Ordinary tests remain offline and do not require installed Agent applications.
For explicit native compatibility validation on POSIX, supply existing executables
and an existing Pi package/Node; the verifier never fetches or installs them:

```sh
python3 scripts/verify-mcp-clients.py --binary /absolute/path/rxd \
  --client pi --pi /absolute/path/pi --node /absolute/path/node \
  --pi-package /absolute/path/node_modules/@earendil-works/pi-coding-agent \
  --client cc --claude /absolute/path/claude \
  --client codex --codex /absolute/path/codex \
  --output /absolute/path/to/new-evidence-directory
```

The output directory must not exist. Omit unselected clients' arguments to verify
a subset, but do not count that as three-client acceptance. Missing paths fail,
not skip-as-pass. The probe copies RxD to a space/Unicode path, creates isolated
HOME/config/cwd fixtures with no inherited provider credentials, and retains
versions, hashes, generated configs, native stdout/stderr and protocol results.
It asserts a real `js_find` result and outside-root `ACCESS_DENIED`, never executes
the analyzed JavaScript, and verifies native engines remain lazy.

- **Pi 1.3.0:** native shell discovery plus deterministic **authored-history
  replay** through the SDK session-wrapped codemode tool and native nested MCP
  pipeline. A schema-valid authored assistant toolCall is imported with the public
  in-memory SessionManager/refreshContext APIs; no provider stream or prompt is
  fabricated/run. Tool validation/hooks and correlated parent execution events
  remain active. Isolated disabled-built-in, `/mcp` extension replacement and
  disabled-server cases prove persisted config is not session availability.
  This is not model-issued use or proof of an overridden real user's session.
- **CC 2.1.301:** generated-config native get/list **health**, paired with a
  **separate recorded executable/argv** stdio initialize/discovery/query/denial.
  It is not a CC model/session invocation.
- **Codex 0.162.1:** native get/list, then supported app-server initialize,
  ephemeral thread/start, MCP status and tool calls. No `turn/start`, inference,
  approval override or trust change; an isolated loopback-only provider prevents
  hosted model catalog access. This is a native no-turn tool route, not inference.

These routes passed locally on macOS ARM64 only; they are not four-platform or
hosted-CI evidence. Client startup is not included in local installer latency.
Upstream contracts: [Pi MCP](https://pi.dev/docs/latest/mcp),
[Pi SDK](https://pi.dev/docs/latest/sdk),
[Pi extensions](https://pi.dev/docs/latest/extensions),
[CC MCP](https://code.claude.com/docs/en/mcp),
[Codex MCP](https://developers.openai.com/codex/mcp), and
[Codex app-server](https://developers.openai.com/codex/app-server).
Version-specific Pi packaged docs/examples and Codex 0.162.1 generated protocol
schemas were also checked; live documentation alone is not a version guarantee.

### Skills and unsupported-client fallback

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
