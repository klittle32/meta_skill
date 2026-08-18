# Fork maintenance: `ms` daily driver

This checkout is a private fork of Meta Skill. Do **not** open a pull request
against `Dicklesworthstone/meta_skill`. The owner does not accept PRs.

Current fork identity:

- Origin (writable): `klittle32/meta_skill`
- Upstream (fetch only, push disabled): `Dicklesworthstone/meta_skill`
- Version: `0.2.1-klittle.1` (prerelease derived from official `0.2.1`)
- Decoder commit kept on `main`: `7c17b443a5577557b6ba59fa7c691996cd3929ff`
- Self-update target: `klittle32/meta_skill` (`src/cli/commands/update.rs`)
- Daily binary: `~/.local/bin/ms-fork`
- PATH `ms`: `~/.local/bin/ms` (shim)

Official `0.2.1` still deserializes `cass export --format json` directly into
`Vec<SessionMessage>`. Raw Claude/Codex records become empty messages, quality
stays at 0, and `ms build --from-cass` returns `no_patterns`. This fork keeps
`decode_cass_export` in `src/cass/decode.rs`.

## Remotes

```bash
git remote -v
# origin    git@github.com:klittle32/meta_skill.git (fetch/push)
# upstream  https://github.com/Dicklesworthstone/meta_skill.git (fetch)
# upstream  DISABLED (push)
```

If `upstream` can still push, disable it:

```bash
git remote set-url --push upstream DISABLED
```

## Daily-driver install

`~/.local/bin` is ahead of `~/.cargo/bin` on this machine’s PATH. Do **not**
make `~/.cargo/bin/ms` the only copy.

```bash
cd ~/Code/meta_skill
cargo build --release --bin ms
install -m 755 target/release/ms ~/.local/bin/ms-fork

cat > ~/.local/bin/ms <<'EOF'
#!/bin/sh
# Daily-driver shim. Official cargo/homebrew ms would drop the cass-export decoder.
# Do not `brew install ms`. Do not `cargo install ms` as the PATH entry.
exec /Users/kyle/.local/bin/ms-fork "$@"
EOF
chmod +x ~/.local/bin/ms
```

`which ms` must be `~/.local/bin/ms`. `ms --version` must print
`ms 0.2.1-klittle.1` (or a later `-klittle.N` suffix).

`strings "$(which ms)"` / `strings ~/.local/bin/ms-fork` must contain
`Failed to decode session export`, not only `Failed to parse session export`.

## What not to do

- Do not `brew install ms` or `brew upgrade ms`. Those bottles are upstream.
- Do not `cargo install --path .` as the daily driver. That writes
  `~/.cargo/bin/ms`, which official `ms update` and later `cargo install`
  will overwrite.
- Do not run official `ms update` against `~/.cargo/bin/ms` while
  `DEFAULT_REPO` still points at Dicklesworthstone.
- Do not reuse a bare upstream version (`0.2.1`). Always suffix
  (`0.2.1-klittle.1`, then `.2`, …).

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
5. Bump the fork prerelease (`0.2.1-klittle.2`, or `0.2.2-klittle.1` after the
   next upstream release).
6. Rebuild and reinstall `~/.local/bin/ms-fork`. Do not publish an official
   GitHub Release that `ms update` could confuse with Dicklesworthstone
   assets unless that release is on `klittle32/meta_skill` and still contains
   the decoder.
7. Verify:

   ```bash
   which ms
   ms --version
   strings ~/.local/bin/ms-fork | rg 'Failed to decode session export'
   ms --robot update --check
   ```

   `update --check` must not offer official `0.2.1` from
   `Dicklesworthstone/meta_skill` as an upgrade.

## Probe

```bash
ms --robot build \
  --auto \
  --from-cass "I compiled and installed ms" \
  --name cass-export-decode-probe \
  --sessions 1 \
  --min-session-quality 0 \
  --output /tmp/ms-restore-probe
```

Expect `status: complete`, quality `> 0` on session `091ab776…`, and at least
one extracted pattern. `no_patterns` / `0%` means the official deserializer
is back.

Rollback of the PATH shim only:

```bash
# official binary was moved aside if present
ls ~/.cargo/bin/ms.upstream-*
```
