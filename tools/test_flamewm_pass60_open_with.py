"""Pass 60 source contracts: standalone chooser, association ownership, no UI blocking."""
from html.parser import HTMLParser
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
MAIN = (ROOT / 'crates/flamewm-explorer/src/main.rs').read_text()
MIME = (ROOT / 'crates/flamewm-explorer/src/mime.rs').read_text()
HTML = (ROOT / 'ui/explorer/explorer.html').read_text()
MANIFEST = (ROOT / 'crates/flamewm-explorer/Cargo.toml').read_text()

class Ids(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids=[]
        self.actions=[]
    def handle_starttag(self, tag, attrs):
        d=dict(attrs)
        if 'id' in d:self.ids.append(d['id'])
        if 'data-action' in d:self.actions.append(d['data-action'])

class OpenWith(unittest.TestCase):
    def test_no_optional_external_chooser_requirement(self):
        self.assertNotIn('Command::new("mimeopen")', MAIN)
        self.assertIn('self.request_open_with(path)', MAIN)
        self.assertIn('flamewm-applications',MANIFEST)
        self.assertIn('entry.launch(&ApplicationLaunchOptions',MIME)
        self.assertIn('This application has no file argument',MIME)

    def test_dedicated_nonblocking_chooser_worker_and_stale_response_fence(self):
        for text in ('ChooserJob::Discover', 'ChooserJob::SetDefault',
                     'name("flamewm-explorer-mime".into())',
                     'if request!=self.chooser_request || !self.chooser_loading {return;}',
                     'WorkerResult::DefaultSaved', 'fn dismiss_chooser(&mut self)'):
            self.assertIn(text,MAIN)
        self.assertIn('fn project_chooser(&self',MAIN)
        self.assertNotIn('Command::new("xdg-mime")',MAIN)

    def test_all_popup_controls_are_unique_and_wired(self):
        nodes=Ids();nodes.feed(HTML)
        self.assertEqual(len(nodes.ids),len(set(nodes.ids)))
        for i in range(6):
            self.assertIn(f'openwith-choice-{i}',nodes.ids)
            self.assertIn(f'explorer.chooser.choice.{i}',nodes.actions)
        for key in ('open','always','cancel','all','prev','next'):
            self.assertIn('explorer.chooser.'+key,nodes.actions)
        for id in ('openwith-dialog','openwith-title','openwith-mime','openwith-page'):
            self.assertIn(id,nodes.ids)

    def test_debian_requires_xdg_mime_service(self):
        control=(ROOT/'packaging/debian/control.in').read_text()
        self.assertIn('xdg-utils',control)
        self.assertIn('shared-mime-info',control)
        lock=(ROOT/'Cargo.lock').read_text()
        self.assertRegex(lock,r'(?s)name = "flamewm-explorer".*?"flamewm-applications"')

    def test_desktop_association_respected_not_custom_database(self):
        self.assertIn('ApplicationCatalog::discover_file_handlers()',MIME)
        self.assertIn('OsStr::new("filetype")',MIME)
        self.assertIn('OsStr::new("default")',MIME)
        self.assertIn('desktop_supports_mime',MIME)
        self.assertIn('xdg-mime',MIME)
        self.assertIn('pub fn local_file_uri',MIME)
        self.assertIn('file:///home/a%20strange/%23%FF.txt',MIME)
        self.assertNotIn('gtk-launch',MIME)

if __name__=='__main__':unittest.main()
