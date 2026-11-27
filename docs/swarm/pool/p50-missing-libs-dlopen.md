# Unit p50-missing-libs-dlopen
STATUS: OPEN

Workspace: native libs linked at start (libpulse, libxkbcommon, libinput, libseat, pam, dbus): list which are hard-linked vs optional; for hard-linked ones that could be missing at runtime find a graceful skip or clear error naming the lib. Rows only plus max 2 edits.
