//! Command-line flag parsing for canary subcommands.

use crate::output::fail;

pub(crate) fn arg(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_owned());
        }
    }
    None
}

pub(crate) fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

pub(crate) fn arg_num<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    arg(args, flag).map_or(default, |v| {
        v.parse()
            .unwrap_or_else(|_| fail(&format!("{flag} must be a number")))
    })
}

pub(crate) fn usage() -> ! {
    eprintln!(
        "flamewm-x11-canary <command> [options]\n\
         commands:\n  \
         list [--name <substr>] [--json]\n  \
         geometry --name <substr> [--json] | --window <id> [--json]\n  \
         map-state --name <substr> [--json] | --window <id>\n  \
         click --name <substr> | --window <id> [--x <n> --y <n>]\n  \
         motion --name <substr> | --window <id> [--x <n> --y <n>]\n  \
         sample --window <id> [--x <n> --y <n> --w <n> --h <n>] [--json]\n  \
         sample-root [--w <n> --h <n>] [--json]\n  \
         stacking [--json]\n  \
         root-geometry [--json]\n  \
         root-shot [--w <n> --h <n>] [--json]\n  \
         click-at --window <id> --x <n> --y <n> --button <1|2|3>\n  \
         wait-mapped --name <substr> [--timeout-secs <n>]\n  \
         profile-startup --name <substr> [--timeout-secs <n>] [--json]\n  \
         profile-pss [--pid <n> | --pattern <substr>] [--json]\n  \
         profile-idle [--secs <n>] [--json]\n  \
         profile-window --name <substr> | --window <id> [--json]\n  \
         sr05 [--name <substr>] [--timeout-secs <n>]\n  \
         sr07 [--parent <substr>] [--json]\n  \
         sr08 [--window <id> | --name <substr>] [--json]\n  \
         sr15 [--name <substr> | --window <id>] [--points x,y;...] [--json]\n  \\
          j12 [--scenario panel|start|selection|desktop-menu|context|chrome|drag-ghost|entry-menu|rename|start-power|short-submenu|audio-network|calendar|sticky|chrome-capture|all] [--json]\n  \
           j13 [--scenario ws-latency|task-activate|status-popup|black-frame|resize-cursors|fixed-size|motif-initial|motif-late|title-maximize|sticky-create|multi-resolution|all] [--json]\n  \
          j06 [--scenario postreg-calendar-300|postreg-start-single-surface|postreg-start-app-click|postreg-desktop-icon-wake|postreg-filemanager-map-latency|postreg-title-centered|postreg-resize-8|postreg-fixed-size|postreg-pixmap-ledger|all] [--json]\n  \
          interaction-perf [--cycles <n>] [--json]
          j07 [--scenario popup|move|resize|chrome|max-snap|workspaces|retention|snap-border|s1|s2|s3|s4|explicit-taskbar-menu|explicit-desktop-settings|explicit-audio-tabs|explicit-osd|explicit-audio-apps|explicit-window-overflow|explicit-window-clamp|all] [--shot-dir <path>] [--json] [--name <substr>] [--window <id>] [--source <substr>] [--repeats <n>] [--run a|b --digest-of <hex> --digest <path>] [--cycles <n>]"
    );
    std::process::exit(2);
}
