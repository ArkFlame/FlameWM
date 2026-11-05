//! Shared fixtures for the async projection test modules.
use super::*;
use flamewm_api::applications::DesktopApplication;

#[cfg(test)]
#[macro_export]
macro_rules! noop_document_overflow {
    () => {
        fn overflow(
            &mut self,
            _: &str,
            _: flamewm_ui_x11::Overflow,
            _: flamewm_ui_x11::Overflow,
        ) -> Result<(), String> {
            Ok(())
        }
    };
}

#[derive(Default)]
pub(crate) struct FakeDoc {
    pub(crate) texts: HashMap<String, String>,
    pub(crate) images: Vec<String>,
    pub(crate) visible: HashMap<String, bool>,
    pub(crate) fills: HashMap<String, UiColor>,
    pub(crate) sizes: HashMap<String, (f32, f32)>,
}

impl UiDocumentAccess for FakeDoc {
    fn text(&mut self, id: &str, text: String) -> Result<(), String> {
        self.texts.insert(id.to_owned(), text);
        Ok(())
    }
    fn image_rgb8(&mut self, id: &str, _image: RuntimeImage) -> Result<(), String> {
        self.images.push(id.to_owned());
        Ok(())
    }
    fn image_rgba8(&mut self, id: &str, _image: RuntimeImage) -> Result<(), String> {
        self.images.push(id.to_owned());
        Ok(())
    }
    fn visible(&mut self, id: &str, v: bool) -> Result<(), String> {
        self.visible.insert(id.to_owned(), v);
        Ok(())
    }
    fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn position(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
        self.sizes.insert(id.to_owned(), (width, height));
        Ok(())
    }
    fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.fills.insert(id.to_owned(), color);
        Ok(())
    }
    fn border(&mut self, _: &str, _: UiColor) -> Result<(), String> {
        Ok(())
    }
    fn background_clear(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn foreground(&mut self, _: &str, _: UiColor) -> Result<(), String> {
        Ok(())
    }
    fn foreground_clear(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn border_clear(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn layer(&mut self, _: &str, _: flamewm_ui_core::style::UiLayer) -> Result<(), String> {
        Ok(())
    }
    crate::noop_document_overflow!();
    fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
        Ok(())
    }
}

pub(super) fn test_image(source: &str) -> RuntimeImage {
    sized_test_image(source, 1)
}

pub(super) fn project_changed(
    view: PanelView<'_>,
    changed: crate::runtime::ChangedDomains,
) -> FakeDoc {
    let mut doc = FakeDoc::default();
    project_panel_for_changed(&mut doc, view, None, &mut TaskIconCache::default(), changed)
        .expect("projects");
    doc
}

pub(super) fn tray_visibility(document: &FakeDoc) -> HashMap<String, bool> {
    document
        .visible
        .iter()
        .filter(|(id, _)| id.starts_with("tray-") || id.as_str() == "clock-button")
        .map(|(id, &visible)| (id.clone(), visible))
        .collect()
}

pub(super) fn project_start_doc(
    model: &StartModel,
    category: StartCategory,
    query: &str,
) -> FakeDoc {
    let mut doc = FakeDoc::default();
    project_start_unified(
        &mut doc,
        model,
        category,
        query,
        &SessionCapabilities::default(),
        None,
    )
    .expect("projects");
    doc
}

pub(super) fn sized_test_image(source: &str, side: u32) -> RuntimeImage {
    RuntimeImage {
        source: source.to_owned(),
        width: side,
        height: side,
        pixels: vec![0; side as usize * side as usize * 4],
    }
}

pub(super) fn start_test_app(id: &str, name: &str, category: &str) -> DesktopApplication {
    DesktopApplication {
        id: flamewm_api::DesktopAppId::new(id),
        name: name.to_owned(),
        generic_name: String::new(),
        comment: String::new(),
        startup_wm_class: String::new(),
        argv: Vec::new(),
        keywords: Vec::new(),
        icon_name: "missing-icon".to_owned(),
        categories: vec![category.to_owned()],
    }
}
