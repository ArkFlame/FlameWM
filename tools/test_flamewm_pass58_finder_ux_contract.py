"""Pass 58 source-level regression guards. They cannot replace Cargo/X11 acceptance."""
import unittest
from pathlib import Path
from html.parser import HTMLParser
ROOT = Path(__file__).resolve().parents[1]

def get(path):
    return (ROOT / path).read_text()

class Parsed(HTMLParser):
    def __init__(self):
        super().__init__(); self.ids={}; self.actions=set()
    def handle_starttag(self,tag,attrs):
        attrs=dict(attrs)
        if 'id' in attrs:self.ids[attrs['id']]=tag
        if 'data-action' in attrs:self.actions.add(attrs['data-action'])

class FinderUX(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.main=get('crates/flamewm-explorer/src/main.rs')
        cls.model=get('crates/flamewm-explorer/src/lib.rs')
        cls.css=get('ui/explorer/explorer.css')
        cls.theme=get('crates/flamewm-explorer/src/theme.rs')
        cls.html=Parsed();cls.html.feed(get('ui/explorer/explorer.html'))

    def test_hidden_single_tab_and_single_page(self):
        self.assertIn('doc.visible("tabs",tab_strip)',self.main)
        self.assertIn('doc.visible("file-pager",tab.page_count()>1)',self.main)
        self.assertIn('"newtab"=>if self.model.tabs.len()<5',self.main)
        self.assertIn('"t" if ctrl',self.main)
        self.assertIn('id="top"',get('ui/explorer/explorer.html'))
        self.assertIn('id="file-pager"',get('ui/explorer/explorer.html'))
        self.assertNotIn('height:84px',self.css)

    def test_live_tinted_zebra_not_gray_literal(self):
        self.assertIn('self.theme.row_alternate()',self.main)
        self.assertIn('pub fn row_alternate(',self.theme)
        self.assertIn('self.surface.r.saturating_add(8)',self.theme)
        self.assertIn('self.theme.selected()',self.main)

    def test_plain_path_home_alias_and_remote_delegation(self):
        self.assertIn('display_location(tab.path(),&home_dir())',self.main)
        self.assertIn('pub fn parse_address(',self.model)
        self.assertIn('"sftp"',self.model)
        self.assertIn('"smb"',self.model)
        self.assertIn('AddressTarget::ExternalUri(uri)',self.main)
        self.assertIn('"xdg-open"',self.main)

    def test_native_open_with_chooser_supersedes_external_mimeopen(self):
        self.assertIn('self.request_open_with(path)',self.main)
        self.assertIn('fn project_chooser(&self',self.main)
        self.assertNotIn('Command::new("mimeopen")',self.main)
        self.assertNotIn('Command::new("which")',self.main)

    def test_ark_and_terminal_are_safely_spawned(self):
        self.assertIn('Command::new("ark").arg(path).spawn()',self.main)
        self.assertIn('Command::new("ark").arg("--add")',self.main)
        self.assertIn('self.spawn_ark(&["--batch","--dialog"]',self.main)
        self.assertIn('Command::new(program).current_dir(&directory).spawn()',self.main)
        for action in ('compress','extract','terminal','newtab'):
            self.assertIn(f'explorer.menu.{action}',self.html.actions)
            self.assertIn(f'menu-{action}',self.html.ids)
        self.assertNotIn('Command::new("sh")',self.main)

    def test_context_menu_filters_entry_and_empty_actions(self):
        self.assertIn('context_on_entry:bool',self.main)
        self.assertIn('self.context_on_entry=false',self.main)
        self.assertIn('self.context_on_entry=true',self.main)
        self.assertIn('doc.visible(&format!("menu-{item}"),enabled)',self.main)
        self.assertIn('MenuMetrics::canonical()',self.main)

    def test_selection_rectangle_and_safe_multiselection(self):
        self.assertIn('selection-rectangle',self.html.ids)
        self.assertIn('self.rubber=Some',self.main)
        self.assertIn('select_vertical_band',self.main)
        self.assertIn('selected_paths: HashSet<PathBuf>',self.model)
        self.assertIn('tab.selected_paths.contains(&entry.path)',self.main)
        self.assertIn('BatchCompleted {report:BatchReport',self.main)
        self.assertIn('self.start_transfer(sources,cut)',self.main)
        self.assertIn('doc.visible("selection-rectangle",false)',self.main)

    def test_wm_uses_canonical_restore_on_client_map(self):
        self.assertIn('self.restore_minimized(event.window)?',get('crates/flamewm-wm-x11/src/wm/events.rs'))

    def test_css_properties_known_to_compiler(self):
        import re
        compiler=get('crates/flamewm-render-compiler/src/compile.rs')
        for body in re.findall(r'\{([^{}]*)\}',self.css):
            for declaration in body.split(';'):
                if ':' not in declaration:continue
                prop=declaration.split(':',1)[0].strip()
                if prop.startswith('--'):continue
                self.assertIn(f'"{prop}"',compiler,msg=f'unsupported CSS property: {prop}')

if __name__=='__main__':unittest.main()
