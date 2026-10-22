//! Consume FlameWM's canonical settings snapshot. No local theme config/polling.
use flamewm_api::settings::{SettingValue,SettingsSnapshot};
use flamewm_ui_core::context_menu::surface_fill;
use flamewm_ui_x11::{UiColor,UiDocumentAccess};

/// FlameWM default accent, the same value as flamewm-settings `DEFAULT_ACCENT`. Used only when no settings snapshot supplies `accent`.
pub const DEFAULT_ACCENT:u32=0xEF4048;
const MAX_FONT_SIZE_OFFSET:i32=4;
const MIN_FONT_SIZE_OFFSET:i32=-2;

#[derive(Clone)]
pub struct ExplorerTheme {accent:UiColor,surface:UiColor,sidebar:UiColor,card:UiColor,accent_alpha:u8,surface_alpha:u8,font_family:Option<String>,font_size_offset:i32,font_bold:bool}
impl Default for ExplorerTheme {
    fn default()->Self {Self {accent:rgb(DEFAULT_ACCENT),surface:rgb(0x202326),sidebar:rgb(0x1b1e20),card:rgb(0x232629),accent_alpha:70,surface_alpha:255,font_family:None,font_size_offset:0,font_bold:false}}
}
/// Dev-only override (`FLAMEWM_EXPLORER_SURFACE_OPACITY=0..=100`) for screenshot runs; invalid values are ignored.
fn dev_surface_alpha_override()->Option<u8> {dev_surface_alpha_override_from(std::env::var("FLAMEWM_EXPLORER_SURFACE_OPACITY").ok().as_deref())}
fn dev_surface_alpha_override_from(value:Option<&str>)->Option<u8> {
    let percent:u16=value?.trim().parse().ok()?;
    (percent<=100).then(||((percent*255)/100) as u8)
}
const fn rgb(value:u32)->UiColor {UiColor{r:(value>>16) as u8,g:(value>>8) as u8,b:value as u8,a:255}}
fn hex(value:&str)->Option<u32> {
    let val=value.strip_prefix('#').unwrap_or(value);
    (val.len()==6&&val.bytes().all(|b|b.is_ascii_hexdigit())).then(||u32::from_str_radix(val,16).ok()).flatten()
}
fn tint(value:u32,offset:i32)->u32 {
    let part=|n:u32| -> u32 {((n as i32+offset).clamp(0,255)) as u32};
    (part((value>>16)&255)<<16)|(part((value>>8)&255)<<8)|part(value&255)
}
impl ExplorerTheme {
    pub fn from_snapshot(snapshot:&SettingsSnapshot)->Self {
        let mut theme=Self::default();
        if let Some(SettingValue::Text(name))=snapshot.values.get("accent") {
            theme.accent=rgb(match name.as_str(){"red"=>DEFAULT_ACCENT,"blue"=>0x3daee9,"purple"=>0x9b59b6,"teal"=>0x1abc9c,"green"=>0x27ae60,"amber"=>0xf39c12,_=>hex(name).unwrap_or(DEFAULT_ACCENT)});
        }
        // accentOpacity tints selection and focus fills; the opaque accent stays for the selected row.
        if let Some(SettingValue::Percent(percent))=snapshot.values.get("accentOpacity") {
            theme.accent_alpha=((u16::from((*percent).min(100))*255)/100) as u8;
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
        // Same surfaceColor/surfaceOpacity mapping as the taskbar and popups (single owner: ui-core).
        // Inner panels take the opaque RGB; only the root fill carries alpha, so it is applied once.
        let fill=surface_fill(snapshot);
        let color=fill.rgb;
        theme.surface=rgb(color);theme.sidebar=rgb(tint(color,-7));theme.card=rgb(tint(color,4));
        theme.surface_alpha=dev_surface_alpha_override().unwrap_or(fill.alpha);
        theme
    }
    pub fn surface(&self)->UiColor {self.surface}
    pub fn sidebar(&self)->UiColor {self.sidebar}
    pub fn card(&self)->UiColor {self.card}
    pub fn row_alternate(&self)->UiColor {
        // Subtle value lift, derived from the live surface rather than fixed CSS gray.
        UiColor{r:self.surface.r.saturating_add(8),g:self.surface.g.saturating_add(8),b:self.surface.b.saturating_add(8),a:255}
    }
    pub fn selection_fill(&self)->UiColor {UiColor{a:self.accent_alpha,..self.accent}}
    pub fn selected(&self)->UiColor {
        let blend=|a:u8,b:u8|->u8 {((u16::from(a)*40+u16::from(b)*60)/100) as u8};
        UiColor{r:blend(self.accent.r,self.surface.r),g:blend(self.accent.g,self.surface.g),b:blend(self.accent.b,self.surface.b),a:255}
    }
    pub fn project(&self,doc:&mut impl UiDocumentAccess)->Result<(),String>{
        for (name,color) in [("--accent",self.accent),("--surface",self.surface),("--sidebar",self.sidebar),("--card",self.card),("--accent-soft",UiColor{a:48,..self.accent})] {
            doc.color_variable(name,color)?;
        }
        // Root fill carries the live surfaceOpacity alpha. Without a compositor the render stack paints the
        // straight colour (no black fallback); see-through needs the shell-owned surface backdrop.
        doc.background("explorer",UiColor{a:self.surface_alpha,..self.surface})?;
        doc.ui_font_defaults(self.font_size_offset as f32,self.font_bold)?;
        if let Some(family)=&self.font_family {doc.font_family(family.clone())?;}
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(live.selection_fill().a,70);
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
        assert_eq!(live.surface.a,255);
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(100));
        assert_eq!(ExplorerTheme::from_snapshot(&settings).surface_alpha,255);
    }

    #[test] fn panels_stay_opaque_while_root_is_translucent() {
        let mut settings=SettingsSnapshot::new(1);
        settings.values.insert("surfaceColor".into(),SettingValue::Text("#445566".into()));
        settings.values.insert("surfaceOpacity".into(),SettingValue::Percent(40));
        let theme=ExplorerTheme::from_snapshot(&settings);
        assert_eq!(theme.surface().a,255);
        assert_eq!(theme.sidebar().a,255);
        assert_eq!(theme.card().a,255);
        assert_eq!(theme.row_alternate().a,255);
        assert_eq!(theme.selected().a,255);
    }

    #[test] fn dev_override_is_clamped_to_percent_range() {
        assert_eq!(dev_surface_alpha_override_from(Some("70")),Some(178));
        assert_eq!(dev_surface_alpha_override_from(Some("101")),None);
        assert_eq!(dev_surface_alpha_override_from(Some("oops")),None);
        assert_eq!(dev_surface_alpha_override_from(None),None);
    }
}
