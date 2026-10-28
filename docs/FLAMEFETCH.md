# Flamefetch (native, one-shot)

The FlameWM Terminal starts its first interactive shell through its own `--flamefetch-shell`
entry. The child prints the configured system summary to the PTY **before** replacing itself
with the user shell (`exec`); the foreground shell keeps correct cursor/scrollback state.
Explicit `-e` commands skip it. Other terminals can call `flamefetch` directly.

Configuration: `~/.config/flamewm/flamefetch.toml` (or
`$XDG_CONFIG_HOME/flamewm/flamefetch.toml`). All keys are optional:

```toml
startup = true
logo = "builtin" # "none" or an absolute path to a custom ASCII text file
logo_color = "#EF4048"
key_color = "#EF4048"
layout = "auto" # "compact" or "full"
palette = true
modules = ["os", "host", "kernel", "uptime", "packages", "shell", "display", "de", "wm", "terminal", "cpu", "gpu", "memory", "swap", "disk"]
```

Command-line arguments: `flamefetch --help`, `--no-logo`, `--logo PATH`, `--layout
compact|auto|full`, `--config PATH`, `--no-color`, `--no-palette`, `--version`.
No subprocesses are spawned by the fetch itself. GPU identification is best-effort;
package counting reads Debian dpkg status when present. No periodic sampling.

The unmodified source logo is installed in `assets/flamefetch/logo.txt`, while
compact layout uses a condensed in-source glyph for narrow terminals. External
custom ASCII files are limited to 32 KiB. Configuration files are limited to 32 KiB.

This shares implementation with `flamewm-terminal`, not Fastfetch's implementation
or complete JSONC schema. The shell startup path deliberately does not modify
`.bashrc`, `.zshrc`, or other user shell configuration.
