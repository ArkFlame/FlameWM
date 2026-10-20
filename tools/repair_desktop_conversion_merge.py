#!/usr/bin/env python3
"""Repair the three desktop-core Convert APIs in an existing FlameWM checkout.

Never replace unrelated source: patch only recognized legacy fragments, retain
backups, and fail closed when another editor changed the relevant function.
The source archive containing this script supplies conversion.rs as payload.
"""
from __future__ import annotations

import argparse
import os
import shutil
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

# `python3 -I` intentionally removes the script directory from sys.path; add only
# this trusted kit directory, never arbitrary checkout paths.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_desktop_conversion_contract import contract_errors

ROOT = Path(__file__).resolve().parent.parent
FILES = (
    'crates/flamewm-desktop-core/src/lib.rs',
    'crates/flamewm-desktop-core/src/file_actions.rs',
    'crates/flamewm-desktop-core/src/conversion.rs',
)
OLD_MENU = '''pub fn entry_context_menu(is_trash: bool) -> Vec<EntryMenuAction> {
    if is_trash {
        vec![EntryMenuAction::EmptyTrash]
    } else {
        vec![
            EntryMenuAction::Rename,
            EntryMenuAction::CreateShortcut,
            EntryMenuAction::Delete,
        ]
    }
}'''
NEW_MENU = '''pub fn entry_context_menu(is_trash: bool, convertible: bool) -> Vec<EntryMenuAction> {
    if is_trash {
        return vec![EntryMenuAction::EmptyTrash];
    }
    let mut rows = vec![EntryMenuAction::Rename, EntryMenuAction::CreateShortcut];
    if convertible {
        rows.push(EntryMenuAction::Convert);
    }
    rows.push(EntryMenuAction::Delete);
    rows
}'''

OLD_MENU_TEST = '''    #[test]
    fn entry_menu_maps_trash_to_empty_trash_only() {
        assert_eq!(entry_context_menu(true), vec![EntryMenuAction::EmptyTrash]);
        assert_eq!(
            entry_context_menu(false),
            vec![
                EntryMenuAction::Rename,
                EntryMenuAction::CreateShortcut,
                EntryMenuAction::Delete
            ]
        );
    }'''
NEW_MENU_TEST = '''    #[test]
    fn entry_menu_maps_trash_to_empty_trash_only() {
        assert_eq!(entry_context_menu(true, true), vec![EntryMenuAction::EmptyTrash]);
        assert_eq!(
            entry_context_menu(false, true),
            vec![
                EntryMenuAction::Rename,
                EntryMenuAction::CreateShortcut,
                EntryMenuAction::Convert,
                EntryMenuAction::Delete
            ]
        );
    }

    #[test]
    fn non_convertible_entries_do_not_offer_convert() {
        assert!(!entry_context_menu(false, false).contains(&EntryMenuAction::Convert));
        assert!(!entry_context_menu(true, true).contains(&EntryMenuAction::Convert));
    }'''



class IncompatibleSource(ValueError):
    pass


def replace_exact(source: str, old: str, new: str, label: str) -> str:
    if new in source:
        return source
    count = source.count(old)
    if count != 1:
        raise IncompatibleSource(f'{label}: expected one recognized legacy declaration, found {count}; refusing to overwrite custom source')
    return source.replace(old, new, 1)


def updated_core(lib: str, actions: str) -> tuple[str, str]:
    lib = replace_exact(lib, 'pub mod file_actions;\n',
                        'pub mod file_actions;\npub mod conversion;\n', 'conversion export')
    actions = replace_exact(actions, '    CreateShortcut,\n    Delete,',
                            '    CreateShortcut,\n    Convert,\n    Delete,', 'Convert enum variant')
    actions = replace_exact(actions, '            Self::CreateShortcut => "Create Shortcut",\n            Self::Delete',
                            '            Self::CreateShortcut => "Create Shortcut",\n            Self::Convert => "Convert",\n            Self::Delete', 'Convert label')
    actions = replace_exact(actions, OLD_MENU, NEW_MENU, 'entry_context_menu implementation')
    # The older core has a unit test that still calls the one-argument API.
    # Update that same contract atomically so `cargo check --all-targets` and
    # `cargo test -p flamewm-desktop-core` do not fail after the release build.
    if 'entry_context_menu(true)' in actions or 'entry_context_menu(false)' in actions:
        actions = replace_exact(actions, OLD_MENU_TEST, NEW_MENU_TEST,
                                'legacy menu tests using one argument')
    return lib, actions


def prepare(target: Path, payload_root: Path = ROOT) -> dict[str, bytes]:
    target = target.resolve(strict=True)
    if not (target / 'Cargo.toml').is_file() or not (target / 'crates/flamewm-desktop/src/main.rs').is_file():
        raise IncompatibleSource(f'{target} is not a FlameWM workspace root (Cargo.toml + desktop main.rs required)')
    if not (target / 'crates/flamewm-desktop-core/Cargo.toml').is_file():
        raise IncompatibleSource('desktop-core manifest is missing')
    lib_path, actions_path, conv_path = (target / rel for rel in FILES)
    for p in (lib_path, actions_path, conv_path):
        if p.is_symlink():
            raise IncompatibleSource(f'symlink source path not supported: {p}')
    try:
        lib = lib_path.read_text(encoding='utf-8')
        actions = actions_path.read_text(encoding='utf-8')
    except (OSError, UnicodeError) as exc:
        raise IncompatibleSource(f'cannot read target sources: {exc}') from exc
    lib2, actions2 = updated_core(lib, actions)
    result: dict[str, bytes] = {}
    if lib2 != lib:
        result[FILES[0]] = lib2.encode('utf-8')
    if actions2 != actions:
        result[FILES[1]] = actions2.encode('utf-8')
    if conv_path.is_file():
        provider = conv_path.read_text(encoding='utf-8')
        if not all(token in provider for token in ('pub const FORMAT_CHOICES', 'pub fn offered_formats', 'pub fn convert_image')):
            raise IncompatibleSource('existing conversion.rs has incompatible public functions; manual reconciliation required')
    else:
        provider_path = payload_root / FILES[2]
        if not provider_path.is_file():
            raise IncompatibleSource(f'repair payload missing: {provider_path}')
        result[FILES[2]] = provider_path.read_bytes()
    return result


def apply(target: Path, plan: dict[str, bytes]) -> Path | None:
    if not plan:
        return None
    root = target.resolve(strict=True)
    # Reconcile after preview: do not apply a stale plan if another agent edited
    # the same source while the user was reviewing the proposed change.
    if prepare(root) != plan:
        raise IncompatibleSource('source changed after planning; rerun the repair')
    backup_dir = root / '.agents' / 'runtime' / (
        'desktop-conversion-repair-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ') + f'-{os.getpid()}')
    backup_dir.mkdir(parents=True, exist_ok=False)
    staged: list[tuple[Path, Path, bool]] = []
    try:
        # Save every replaced file before the first mutation.
        for rel in plan:
            path = root / rel
            existed = path.exists()
            if existed:
                backup = backup_dir / rel
                backup.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, backup)
            staged.append((path, backup_dir / rel, existed))
        for rel, content in plan.items():
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            old_mode = path.stat().st_mode & 0o777 if path.exists() else 0o644
            fd, temp = tempfile.mkstemp(prefix='.flamewm-repair-', dir=path.parent)
            try:
                with os.fdopen(fd, 'wb') as f:
                    f.write(content)
                    f.flush()
                    os.fsync(f.fileno())
                os.chmod(temp, old_mode)
                os.replace(temp, path)
            finally:
                if os.path.exists(temp):
                    os.unlink(temp)
        errors = contract_errors(root)
        if errors:
            raise IncompatibleSource('post-repair contract failed: ' + '; '.join(errors))
    except Exception:
        for path, backup, existed in reversed(staged):
            if existed and backup.is_file():
                shutil.copy2(backup, path)
            elif not existed:
                path.unlink(missing_ok=True)
        raise
    return backup_dir


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', type=Path, default=Path.cwd(), help='FlameWM Git checkout root')
    parser.add_argument('--apply', action='store_true', help='Apply with backups (default: preview)')
    args = parser.parse_args()
    try:
        plan = prepare(args.target)
        if not plan:
            errors = contract_errors(args.target.resolve())
            if errors:
                raise IncompatibleSource('contract mismatch despite no supported edits: ' + '; '.join(errors))
            print('OK: checkout already has the complete desktop Convert API; no changes made.')
            return 0
        print('Target:', args.target.resolve())
        for rel in plan:
            print('  ' + ('ADD  ' if not (args.target / rel).exists() else 'EDIT ') + rel)
        if not args.apply:
            print('DRY RUN: add --apply to safely apply with a backup. Nothing changed.')
            return 0
        backups = apply(args.target, plan)
        print('OK: converted desktop-core API merged into checkout.')
        print('Backups:', backups)
        print('Next: CARGO_BUILD_JOBS=2 cargo check -p flamewm-desktop --all-targets')
        return 0
    except (IncompatibleSource, OSError, UnicodeError) as exc:
        print('FLAMEWM DESKTOP REPAIR BLOCKED:', exc, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
