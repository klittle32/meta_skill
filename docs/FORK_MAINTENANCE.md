# Fork maintenance: `ms` daily driver

This checkout is a private fork of Meta Skill. Do **not** open a pull request
against `Dicklesworthstone/meta_skill`. The owner does not accept PRs. The
policy matches cass: we keep a writable fork, a version suffix, a retargeted
updater, and a `~/.local/bin` daily driver that official installers cannot
silently replace.

## Current identity (2026-08-18)

| Item | Value |
|---|---|
| Origin (writable) | `klittle32/meta_skill` |
| Upstream (fetch only) | `Dicklesworthstone/meta_skill` (`push` = `DISABLED`) |
| Checkout | `~/Code/meta_skill` on `main` |
| Version | `0.2.1-klittle.1` (prerelease from official `0.2.1`) |
| Decoder commit | `7c17b443a5577557b6ba59fa7c691996cd3929ff` |
| Upstream absorb | `0976f86` merge of official `v0.2.1` (`b39db18`, includes `#172`) |
| Fork identity commit | `1e553de` |
| Self-update target | `klittle32/meta_skill` (`DEFAULT_REPO` in `src/cli/commands/update.rs`) |
| Real binary | `~/.local/bin/ms-fork` |
| PATH `ms` | `~/.local/bin/ms` (shim → `ms-fork`) |
| Local note | `~/.local/share/ms/FORK_INSTALL.txt` |

`Cargo.toml` `repository` / `homepage` also point at `klittle32/meta_skill`.

## Why this fork exists

`ms build --from-cass` calls:

```text
cass export <session_path> --format json --include-tools
```

CASS 0.6.x still emits raw provider records when the source `.jsonl` is on
disk:

- Claude Code: conversation under `message.role` / `message.content`
- Codex: top-level `type == "response_item"`, item kind under `payload.type`
- Indexed-only fallback: flat `{role, content}` plus inline `[Tool: …]`

Official `ms` (through `v0.2.1`) does `serde_json::from_slice` into
`Vec<SessionMessage>`. Unknown fields default away. Raw records become empty
messages, quality stays at `0`, and the build returns `no_patterns`. That is
upstream `#114`, closed against the wrong export shape.

This fork keeps `decode_cass_export` in `src/cass/decode.rs`.
`CassClient::get_session` must call that decoder. Do not restore the
normalized-only deserialize loop.

## What we did

### 2026-08-15 — decoder

1. Confirmed remotes: `origin` = fork, `upstream` push disabled.
2. Implemented record-level dispatch for the three export shapes.
3. Fast-forwarded `fix/cass-export-shape-decoder` onto `main` and pushed
   `origin/main` as `7c17b44`.
4. Installed with `cargo install --path ~/Code/meta_skill` at 18:31. That
   wrote `~/.cargo/bin/ms` (`0.2.0` with the decoder). This was the wrong
   long-term path (see the clobber below).

Verified then: `review-surface` from `agentic-enrichment` reached
`status: complete`, 3 sessions (quality 0.65 / 0.15 / 0.65), 7 patterns.

### 2026-08-18 — official 0.2.1 overwrote PATH

Kyle installed official `ms 0.2.1` (GitHub release published that day). It
replaced `~/.cargo/bin/ms` (mtime 15:49). `ms update --check` said the
machine was current because the updater still targeted
`Dicklesworthstone/meta_skill`.

Same session `091ab776-f8e6-4961-adb6-ce05bbcbdf18`:

| Binary | Quality | Result |
|---|---|---|
| Official PATH `ms 0.2.1` | 0% | No patterns |
| `~/Code/meta_skill/target/release/ms` (patched 0.2.0) | 40% | 5 patterns |

Official `0.2.1` notes are only `ms show` id/name (`#172`) plus deps. No
decoder. `upstream/main` at `b39db18` does not contain `7c17b44`.

### 2026-08-18 — restore, absorb 0.2.1, cass-style install

1. Confirmed `7c17b44` still on `origin/main` and `decode_cass_export` present.
2. Merged `upstream/main` (`v0.2.1`) into fork `main` as `0976f86`. No
   `src/cass/client.rs` conflict; decoder kept.
3. Set version `0.2.1-klittle.1`. Do not reuse a bare upstream version.
4. Pointed `DEFAULT_REPO`, `repository`, and `homepage` at
   `klittle32/meta_skill`.
5. Built `cargo build --release --bin ms` and installed:

   - `~/.local/bin/ms-fork` — real fork binary
   - `~/.local/bin/ms` — 253-byte shim
   - `~/.local/share/ms/FORK_INSTALL.txt` — local reminder

   `~/.local/bin` is ahead of `~/.cargo/bin` on this machine’s PATH, so
   official Cargo/Homebrew writes to `~/.cargo/bin/ms` no longer win
   `which ms`.
6. Pushed identity commit `1e553de` to `origin/main`. No upstream PR.

Verified after restore:

```text
which ms                  → /Users/kyle/.local/bin/ms
ms --version              → ms 0.2.1-klittle.1
strings ms-fork           → Failed to decode session export
ms --robot update --check → update_available: false, latest_version: null
```

Probe (from `/tmp`, not `~/Code/meta_skill`):

```text
query: I compiled and installed ms
session: 091ab776-f8e6-4961-adb6-ce05bbcbdf18
quality: 0.40
patterns_extracted: 5
status: complete
```

A first probe from `~/Code/meta_skill` failed with
`table skills already exists` on that tree’s `.ms/ms.db`. That is a local
store migration problem, not a decoder regression. Run builds from a directory
without a broken `.ms/` (for example `/tmp`).

DCG blocked `mv ~/.cargo/bin/ms`. Official `0.2.1` may still sit at
`~/.cargo/bin/ms` until you run the removal commands below.

## Remotes

```bash
git remote -v
# origin    git@github.com:klittle32/meta_skill.git (fetch/push)
# upstream  https://github.com/Dicklesworthstone/meta_skill.git (fetch)
# upstream  DISABLED (push)
```

If `upstream` can still push:

```bash
git remote set-url --push upstream DISABLED
```

## Daily-driver install

Do **not** make `~/.cargo/bin/ms` the only copy.

```bash
cd ~/Code/meta_skill
cargo build --release --bin ms
install -m 755 target/release/ms ~/.local/bin/ms-fork

cat > ~/.local/bin/ms <<'EOF'
#!/bin/sh
# Daily-driver shim. Official cargo/homebrew ms would drop the cass-export decoder.
# See ~/Code/meta_skill/docs/FORK_MAINTENANCE.md
# Do not `brew install ms`. Do not `cargo install ms` as the PATH entry.
exec /Users/kyle/.local/bin/ms-fork "$@"
EOF
chmod +x ~/.local/bin/ms
```

Checks:

```bash
which ms
# /Users/kyle/.local/bin/ms

ms --version
# ms 0.2.1-klittle.1   (or a later -klittle.N)

strings ~/.local/bin/ms-fork | rg 'Failed to decode session export'
```

## Remove official `ms` so only the fork remains

Homebrew was not installed for `ms` on the cutover machine. Official leftover
is Cargo’s `~/.cargo/bin/ms`. Leave `~/.local/bin/ms` and `ms-fork` alone.

```bash
which -a ms
ms --version
# expect /Users/kyle/.local/bin/ms and ms 0.2.1-klittle.1

cargo uninstall ms
rm -f ~/.cargo/bin/ms.bak-0.2.0-aug10
rm -f ~/.cargo/bin/ms.upstream-0.2.1

which -a ms
ls -l ~/.local/bin/ms ~/.local/bin/ms-fork ~/.cargo/bin/ms
ms --version
strings "$(which ms)" | rg 'Failed to decode session export'
```

After that, `which -a ms` should print only `/Users/kyle/.local/bin/ms`.

## What not to do

These put official `ms` back on `~/.cargo/bin`:

```bash
# do not run
brew install ms
brew upgrade ms
cargo install ms
cargo install --path ~/Code/meta_skill
```

Also:

- Do not open issues or PRs on `Dicklesworthstone/meta_skill`.
- Do not run official `ms update` against `~/.cargo/bin/ms` if `DEFAULT_REPO`
  has drifted back to Dicklesworthstone. Official `0.2.1` is a downgrade of
  the decoder even when semver looks newer than `0.2.0`.
- Do not reuse a bare upstream version (`0.2.1`). Always suffix
  (`0.2.1-klittle.1`, then `.2`, or `0.2.2-klittle.1` after the next
  upstream release).
- Do not treat `~/Code/meta_skill/.ms/ms.db` as healthy until the
  `table skills already exists` migration is investigated separately.

`ms update --check` on the fork should report `update_available: false`
until `klittle32/meta_skill` has its own GitHub Release. That is correct.
Do not “fix” it by pointing the updater back at Dicklesworthstone.

## Pulling upstream

1. `git fetch upstream --tags --prune`
2. Merge `upstream/main` into `main`. Prefer merge over rebase once the fork
   commit is on `origin/main`.
3. If `src/cass/client.rs` conflicts, keep `decode_cass_export`. Do not restore
   `serde_json::from_slice` into `Vec<SessionMessage>`.
4. Preserve:
   - `Cargo.toml` `version` suffix and `repository`/`homepage`
   - `DEFAULT_REPO = "klittle32/meta_skill"`
   - `src/cass/decode.rs`
5. Bump the fork prerelease.
6. Rebuild and reinstall `~/.local/bin/ms-fork` only. Do not
   `cargo install --path .`.
7. Verify:

   ```bash
   which ms
   ms --version
   strings ~/.local/bin/ms-fork | rg 'Failed to decode session export'
   ms --robot update --check
   ```

## Probe

Run from a directory without a broken project `.ms/` store:

```bash
cd /tmp
rm -rf /tmp/ms-restore-probe

ms --robot build \
  --auto \
  --from-cass "I compiled and installed ms" \
  --name cass-export-decode-probe \
  --sessions 1 \
  --min-session-quality 0 \
  --min-sessions 1 \
  --min-patterns 1 \
  --min-confidence 0 \
  --output /tmp/ms-restore-probe

jq '{
  sessions_used: [.sessions_used[] | {id, quality_score}],
  patterns_extracted
}' /tmp/ms-restore-probe/build-manifest.json
```

Expect session `091ab776-f8e6-4961-adb6-ce05bbcbdf18`, quality `> 0`, and at
least one pattern. `no_patterns` / `0%` means the official deserializer is
back.

Default gates are `--min-sessions 3` and `--min-patterns 5`. A one-session
probe needs the lowered gates above or it will fail after a successful
decode.
