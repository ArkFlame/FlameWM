# flamewm-remote-fs: explorer integration

Owner: this crate. The explorer (`flamewm-explorer`) is its only caller and must not reach into
ssh, ls, or cat itself. Nothing here touches UI or WM state.

## Shape

- Create one `RemoteFs` per process: `RemoteFs::new(Config::default())`. It starts `workers`
  (default 4) std threads. Requests are queued; every call returns at once.
- Parse user input with `Url::parse("sftp://user@host:port/path")`. Schemes: `sftp`, `ssh`, `fish`
  (supported), `smb`, `ftp` (parse only, `Error::UnsupportedProtocol`).
- Empty path = remote home. `fish://host/~/x` = `~/x`. `Url::join`, `parent`, `file_name` build
  breadcrumbs. `Url::remote_path()` is internal, not for display.
- Poll replies from the UI loop with `try_recv`/`try_take` (never block the UI thread). `wait()` is
  for workers and tests only.

## Calls

| Need | Call | Result |
| --- | --- | --- |
| Directory view | `list_dir(&url, &cancel)` | `Receiver<ListEvent>`: `Chunk(Vec<Entry>)`* then `Done{total}` or `Failed` |
| Properties | `stat(&url)` | `Reply<Entry>` |
| Home crumb | `home(&url)` | `Reply<Url>` absolute home URL |
| New folder | `mkdir(&url)` | `Reply<()>` |
| Delete | `remove(&url, recursive)` | `Reply<()>`; refuses `/`, home, `.`, `..` |
| Rename / move | `rename(&from, &to)` | `Reply<()>`; never overwrites (`AlreadyExists`) |
| Copy to local | `download(&url, &local_path, &cancel)` | `Receiver<TransferEvent>`: `Progress{done,total}`, `Done{bytes}`, `Failed` |
| Copy to remote | `upload(&local_path, &url, &cancel)` | same events; atomic temp-file rename |
| Host trust | `probe_host_key(&url)` then `trust_host_key(&prompt)` | see below |
| After external change | `invalidate(&url)` | next listing re-fetches |

`Entry { name, kind: File|Dir|Symlink|Other, size, mtime (unix s, 0 unknown), mode, link_target }`.

Listings: the first chunk is at most 64 rows, then 512 rows per chunk. Paint each chunk as it arrives.
Listings are cached for `Config::cache_ttl` (2 s). Your own mutations invalidate the target and its
parent automatically.

Cancel: `Cancel::new()`, call `cancel()` on navigation away or a dialog close. The job kills its
ssh child within ~25 ms and emits `Failed(Error::Cancelled)`. A cancelled download removes its
`.flamewm-part` file; a cancelled upload removes the remote temp file.

## Errors (`Error`, all typed, never contain secrets)

- `HostKeyUnknown{host,port}`: show a dialog. Call `probe_host_key(&url)`, display
  `prompt.fingerprint_sha256` and `prompt.key_type`, and call `trust_host_key(&prompt)` only on the
  user's explicit confirmation. The fingerprint is unauthenticated, so the user must compare it
  against the server's real fingerprint. Then retry.
- `HostKeyChanged{..}`: show a hard error. There is no override in this crate.
- `AuthFailed{host, password_possible}`: key and agent auth only (`BatchMode=yes`). Tell the user
  to add a key or load an agent. `password_possible` means the server offered a password method
  that this client does not answer. No password prompt is ever shown or stored.
- `Timeout`, `ConnectionLost`: show a retry action. The pool drops the connection so the retry
  re-handshakes.
- `NotFound`, `PermissionDenied`, `AlreadyExists`, `IsDirectory`, `Remote{..}`, `Io`,
  `InvalidUrl`, `UnsupportedProtocol`: show the message.

## Transport notes for maintainers

- Auth and host keys are OpenSSH's job: `~/.ssh/known_hosts`, `~/.ssh/config`, the agent.
  `StrictHostKeyChecking=yes` means an unknown host is never accepted silently.
- One ControlMaster socket per (user, host, port) under `$XDG_RUNTIME_DIR/flamewm-rfs/` (or the
  temp dir). `ServerAliveInterval=15` and `ServerAliveCountMax=3` detect dead links in about 45 s.
  `ControlPersist=600` keeps the master.
- Both `sftp` and `fish` run remote `ls -lAn --time-style=+%s`, `cat`, `mv`, `mkdir`, `rm` through
  the shell. The `sftp` scheme does not use the SFTP subsystem. One round trip per directory, and
  it works on hosts that disable the subsystem but allow a shell. Remote hosts need POSIX sh and
  GNU coreutils.
- `FLAMEWM_RFS_SSH` overrides the ssh binary (tests use a shim).
- Not implemented: smb:// and ftp:// backends (gvfs or FUSE fallback), password auth, SFTP
  subsystem, rename across hosts, resumable transfers, background keepalive thread (ServerAlive
  covers the link).

## Tests

- `cargo test -p flamewm-remote-fs`: unit tests (URL, parsers, pool, cache, shell quoting,
  error mapping) and `tests/shim_end_to_end.rs` (full async path through a local shell shim).
- `FLAMEWM_RFS_TEST_URL=sftp://user@host/ cargo test -p flamewm-remote-fs --test real_host -- --nocapture`:
  read-only home plus one listing against a real host. Requires the key or agent to already work and
  the host key to already be in known_hosts.
