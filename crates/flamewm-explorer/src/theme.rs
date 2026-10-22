//! Consume FlameWM's canonical settings snapshot. No local theme config/polling.
//! Every colour derives from the live `surfaceColor` and `surfaceOpacity`: the root fills once with the surface
//! (translucent over the blurred wallpaper when opacity is below 100), darker panels draw nothing, and lighter
//! elements are a white wash over that surface. No grey literal is painted anywhere.
use std::path::Path;

use flamewm_api::settings::{SettingValue,SettingsSnapshot};
use flamewm_ui_core::context_menu::surface_fill;
use flamewm_ui_x11::backdrop::WindowBackdrop;
use flamewm_ui_x11::{UiColor,UiDocumentAccess};

/// FlameWM default accent, the same value as flamewm-settings `DEFAULT_ACCENT`. Used only when no settings snapshot supplies `accent`.
pub const DEFAULT_ACCENT:u32=0xEF4048;
/// Shared defaults (flamewm-ui-core `DEFAULT_SURFACE_OPACITY`, shell/desktop `surfaceBlurPercent`).
const DEFAULT_SURFACE_OPACITY:u8=50;
const DEFAULT_BLUR_PERCENT:u8=80;
/// Light wash over the surface for rows, headers, fields and cards (10%).
const WASH_ALPHA:u8=26;
/// Accent opacity for selection and highlight fills (50%).
const SELECTION_ALPHA:u8=0x80;
/// Stronger wash for hover and pressed states (25%).
const HOVER_ALPHA:u8=64;
/// Menus and dialogs read as the surface, a little more opaque than the root for legibility.
const MENU_MIN_ALPHA:u8=230;
/// Secondary text: white at roughly 65%, so it stays legible over any surface colour.
const MUTED_ALPHA:u8=166;
const MAX_FONT_SIZE_OFFSET:i32=4;
const MIN_FONT_SIZE_OFFSET:i32=-2;

#[derive(Clone)]
pub struct ExplorerTheme {accent:UiColor,surface:UiColor,surface_alpha:u8,font_family:Option<String>,font_size_offset:i32,font_bold:bool}
impl Default for ExplorerTheme {
    fn default()->Self {Self::from_snapshot(&SettingsSnapshot::new(0))}
}
/// Dev-only override (`FLAMEWM_EXPLORER_SURFACE_OPACITY=0..=100`) for screenshot runs; invalid values are ignored.
fn dev_surface_percent_override()->Option<u8> {dev_surface_percent_override_from(std::env::var("FLAMEWM_EXPLORER_SURFACE_OPACITY").ok().as_deref())}
fn dev_surface_percent_override_from(value:Option<&str>)->Option<u8> {
    let percent:u16=value?.trim().parse().ok()?;
    (percent<=100).then_some(percent as u8)
}
/// Surface opacity percent: the dev override when set, else the live `surfaceOpacity` (shared default 50).
pub fn surface_opacity_percent(snapshot:&SettingsSnapshot)->u8 {
    dev_surface_percent_override().unwrap_or_else(||match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(percent))=>(*percent).min(100),
        _=>DEFAULT_SURFACE_OPACITY,
    })
}
/// Backdrop for the window: the shared `wallpaper`/`wallpaperFit` settings, the surface opacity and `surfaceBlurPercent`.
/// Installed at startup; the window API has no live swap, so later snapshot changes update fills but not the wallpaper layer.
pub fn window_backdrop(snapshot:&SettingsSnapshot,packaged_root:&Path)->WindowBackdrop {
    let text=|key:&str|match snapshot.values.get(key) {Some(SettingValue::Text(value))=>value.clone(),_=>String::new()};
    let fit=text("wallpaperFit");
    let blur=match snapshot.values.get("surfaceBlurPercent") {
        Some(SettingValue::Percent(percent))=>(*percent).min(100),
        _=>DEFAULT_BLUR_PERCENT,
    };
    WindowBackdrop {
        wallpaper:text("wallpaper"),
        fit:if fit.trim().is_empty() {"fill".to_owned()} else {fit},
        packaged_root:packaged_root.to_path_buf(),
        opacity_percent:surface_opacity_percent(snapshot),
        blur_percent:blur,
    }
}
const fn rgb(value:u32)->UiColor {UiColor{r:(value>>16) as u8,g:(value>>8) as u8,b:value as u8,a:255}}
const fn white(alpha:u8)->UiColor {UiColor{r:255,g:255,b:255,a:alpha}}
fn hex(value:&str)->Option<u32> {
    let val=value.strip_prefix('#').unwrap_or(value);
    (val.len()==6&&val.bytes().all(|b|b.is_ascii_hexdigit())).then(||u32::from_str_radix(val,16).ok()).flatten()
}
impl ExplorerTheme {
    pub fn from_snapshot(snapshot:&SettingsSnapshot)->Self {
        let mut theme=Self {accent:rgb(DEFAULT_ACCENT),surface:rgb(0),surface_alpha:0,font_family:None,font_size_offset:0,font_bold:false};
        if let Some(SettingValue::Text(name))=snapshot.values.get("accent") {
            theme.accent=rgb(match name.as_str(){"red"=>DEFAULT_ACCENT,"blue"=>0x3daee9,"purple"=>0x9b59b6,"teal"=>0x1abc9c,"green"=>0x27ae60,"amber"=>0xf39c12,_=>hex(name).unwrap_or(DEFAULT_ACCENT)});
        }
        if let Some(SettingValue::Text(name))=snapshot.values.get("fontFamily") {
            let name=name.trim();
            theme.font_family=(!name.is_empty()).then(||name.to_owned());
        }
        if let Some(SettingValue::Integer(offset))=snapshot.values.get("fontSizeOffset") {
            theme.font_size_offset=(*offset).clamp(i64::from(MIN_FONT_SIZE_OFFSET),i64::from(MAX_FONT_SIZE_OFFSET)) as i32;
        }
        if let Some(SettingValue::Boolean(bold))=snapshot.values.get("fontBold") {
            theme.font_bold=*bold;
        }
        // Same surfaceColor mapping as the taskbar and popups (single owner: ui-core). The alpha is the
        // opacity percent (dev override included) so it matches the backdrop's opacity exactly.
        let fill=surface_fill(snapshot);
        theme.surface=rgb(fill.rgb);
        let percent=surface_opacity_percent(snapshot);
        theme.surface_alpha=((u16::from(percent)*255)/100) as u8;
        theme
    }
    /// Root fill: the surface once, translucent when the window backdrop shows the wallpaper through it.
    pub fn root(&self)->UiColor {UiColor{a:self.surface_alpha,..self.surface}}
    /// Darker panels (sidebar, toolbar, status strip, list, dark zebra rows): no fill, the root shows through.
    pub fn clear(&self)->UiColor {white(0)}
    /// Lighter elements: a white wash over the surface (headers, cards, fields, tabs, zebra rows).
    pub fn card(&self)->UiColor {white(WASH_ALPHA)}
    pub fn row_alternate(&self)->UiColor {white(WASH_ALPHA)}
    pub fn hover(&self)->UiColor {white(HOVER_ALPHA)}
    /// Menus and dialogs: the surface colour, opaque enough to read over busy wallpaper.
    pub fn menu(&self)->UiColor {UiColor{a:self.surface_alpha.max(MENU_MIN_ALPHA),..self.surface}}
    /// Secondary text. Text colour ignores alpha, so this is white blended over the surface, opaque.
    pub fn muted(&self)->UiColor {
        let mix=|c:u8|->u8 {((u16::from(c)*(255-u16::from(MUTED_ALPHA))+255*u16::from(MUTED_ALPHA))/255) as u8};
        UiColor{r:mix(self.surface.r),g:mix(self.surface.g),b:mix(self.surface.b),a:255}
    }
    /// Selection and highlight fills use the accent at 50% opacity, blended over the surface.
    pub fn selection_fill(&self)->UiColor {UiColor{a:SELECTION_ALPHA,..self.accent}}
    pub fn selected(&self)->UiColor {
        // Accent at 50% over the surface, opaque: the selected row stays readable at any opacity.
        let blend=|a:u8,b:u8|->u8 {((u16::from(a)*u16::from(SELECTION_ALPHA)+u16::from(b)*(255-u16::from(SELECTION_ALPHA)))/255) as u8};
        UiColor{r:blend(self.accent.r,self.surface.r),g:blend(self.accent.g,self.surface.g),b:blend(self.accent.b,self.surface.b),a:255}
    }
    pub fn project(&self,doc:&mut impl UiDocumentAccess)->Result<(),String>{
        for (name,color) in [("--accent",self.accent),("--accent-soft",UiColor{a:48,..self.accent}),("--sidebar",self.clear()),
            ("--card",self.card()),("--line",self.card()),("--hover",self.hover()),("--menu",self.menu()),("--muted",self.muted()),("--text",white(245))] {
            doc.color_variable(name,color)?;
        }
        // The root fill carries the live surfaceOpacity alpha, applied once; children draw no second surface.
        doc.background("explorer",self.root())?;
        doc.ui_font_defaults(self.font_size_offset as f32,self.font_bold)?;
        if let Some(family)=&self.font_family {doc.font_family(family.clone())?;}
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn muted_text_is_opaque_and_dimmer_than_full_white() {
        let muted=ExplorerTheme::default().muted();
        assert_eq!(muted.a,255);
        assert!(muted.r<245 && muted.r>ExplorerTheme::default().surface.r);
    }
    #[test] fn unknown_values_preserve_defaults() {
        let mut settings=SettingsSnapshot::new(1);
        settings.values.insert("surfaceColor".into(),SettingValue::Text("oops".into()));
        let mapped=ExplorerTheme::from_snapshot(&settings);
        assert_eq!(mapped.surface.r,(surface_fill(&settings).rgb>>16) as u8);
    }

    #[test] fn accent_defaults_to_flamewm_default_and_follows_snapshot() {
        // No snapshot (or no accent key) keeps FlameWM's real default accent, never a private literal.
        let defaults=ExplorerTheme::from_snapshot(&SettingsSnapshot::new(1));
        assert_eq!((defaults.accent.r,defaults.accent.g,defaults.accent.b),(0xEF,0x40,0x48));
        let mut settings=SettingsSnapshot::new(2);
        settings.values.insert("accent".into(),SettingValue::Text("#3DAEE9".into()));
        let live=ExplorerTheme::from_snapshot(&settings);
        assert_eq!((live.accent.r,live.accent.g,live.accent.b),(0x3D,0xAE,0xE9));
        // Selection fills are the accent at 50% (0x80), regardless of the accentOpacity setting.
        assert_eq!(live.selection_fill().a,0x80);
        assert_eq!(live.selection_fill().r,0x3D);
        assert_eq!(live.selected().a,255);
        settings.values.insert("accent".into(),SettingValue::Text("nonsense".into()));
        let bad=ExplorerTheme::from_snapshot(&settings);
        assert_eq!((bad.accent.r,bad.accent.g,bad.accent.b),(0xEF,0x40,0x48));
    }

    #[test] fn surface_opacity_font_and_size_offset_map_from_snapshot() {
        let mut settings=SettingsSnapshot::new(1);
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(50));
        settings.values.insert("fontFamily".into(),SettingValue::Text("  Noto Sans  ".into()));
        settings.values.insert("fontSizeOffset".into(),SettingValue::Integer(9));
        let mapped=ExplorerTheme::from_snapshot(&settings);
        assert_eq!(mapped.surface_alpha,127);
        assert_eq!(mapped.font_family.as_deref(),Some("Noto Sans"));
        assert_eq!(mapped.font_size_offset,4);
        settings.values.insert("fontSizeOffset".into(),SettingValue::Integer(-9));
        settings.values.insert("fontFamily".into(),SettingValue::Text("   ".into()));
        let low=ExplorerTheme::from_snapshot(&settings);
        assert_eq!(low.font_size_offset,-2);
        assert_eq!(low.font_family,None);
        // Missing surfaceOpacity uses the shared taskbar default (50%), not an opaque literal.
        let defaults=ExplorerTheme::from_snapshot(&SettingsSnapshot::new(1));
        assert_eq!(defaults.surface_alpha,127);
    }

    #[test] fn root_alpha_follows_shared_surface_fill_and_100_is_opaque() {
        let mut settings=SettingsSnapshot::new(1);
        settings.values.insert("surfaceColor".into(),SettingValue::Text("#445566".into()));
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(70));
        let live=ExplorerTheme::from_snapshot(&settings);
        let fill=surface_fill(&settings);
        assert_eq!(live.surface_alpha,fill.alpha);
        assert_eq!(live.surface_alpha,178);
        assert_eq!((live.surface.r,live.surface.g,live.surface.b),(0x44,0x55,0x66));
        assert_eq!(live.root().a,178);
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(100));
        assert_eq!(ExplorerTheme::from_snapshot(&settings).surface_alpha,255);
    }

    #[test] fn children_never_stack_a_second_surface_alpha() {
        let mut settings=SettingsSnapshot::new(1);
        settings.values.insert("surfaceColor".into(),SettingValue::Text("#445566".into()));
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(40));
        let theme=ExplorerTheme::from_snapshot(&settings);
        // Panels draw no surface; lighter elements are a 10% white wash, hover 25%, menus near-opaque surface.
        assert_eq!(theme.clear().a,0);
        assert_eq!(theme.card(),UiColor{r:255,g:255,b:255,a:26});
        assert_eq!(theme.hover().a,64);
        assert!(theme.menu().a>=230);
        assert_eq!((theme.menu().r,theme.menu().g,theme.menu().b),(0x44,0x55,0x66));
    }

    #[test] fn window_backdrop_mirrors_surface_opacity_blur_and_wallpaper_keys() {
        let root=Path::new("/packaged");
        let defaults=window_backdrop(&SettingsSnapshot::new(1),root);
        assert_eq!((defaults.opacity_percent,defaults.blur_percent),(50,80));
        assert_eq!((defaults.fit.as_str(),defaults.wallpaper.as_str()),("fill",""));
        let mut settings=SettingsSnapshot::new(2);
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(70));
        settings.values.insert("surfaceBlurPercent".into(),SettingValue::Percent(0));
        settings.values.insert("wallpaper".into(),SettingValue::Text("/w.png".into()));
        settings.values.insert("wallpaperFit".into(),SettingValue::Text("center".into()));
        let live=window_backdrop(&settings,root);
        assert_eq!((live.opacity_percent,live.blur_percent),(70,0));
        assert_eq!((live.wallpaper.as_str(),live.fit.as_str()),("/w.png","center"));
        assert!(live.is_translucent());
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(100));
        assert!(!window_backdrop(&settings,root).is_translucent());
    }

    #[test] fn dev_override_is_clamped_to_percent_range() {
        assert_eq!(dev_surface_percent_override_from(Some("70")),Some(70));
        assert_eq!(dev_surface_percent_override_from(Some("101")),None);
        assert_eq!(dev_surface_percent_override_from(Some("oops")),None);
        assert_eq!(dev_surface_percent_override_from(None),None);
    }
}
