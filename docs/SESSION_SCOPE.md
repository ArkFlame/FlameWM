# Session switching is out of scope

FlameWM is a desktop environment. It does not switch the desktop session to Xfce, KDE Plasma, or any other environment at runtime.

- Sessions are chosen by the display manager (login screen), which starts one session per login.
- FlameWM provides only its own session. Settings, Start, the panel, and the dock do not offer a "switch desktop" action.
- Reference desktops in `.vendor/**` are read-only study material. Their session switchers are not ported.

Reason: a session switcher would make FlameWM start or stop other desktop stacks from inside the shell. That crosses the ownership boundary in `docs/ARCHITECTURE.md`, where the display manager and session owners control login and session lifetime.
