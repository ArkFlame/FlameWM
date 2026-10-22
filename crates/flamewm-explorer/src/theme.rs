//! Consume FlameWM's canonical settings snapshot. No local theme config/polling.
use flamewm_api::settings::{SettingValue,SettingsSnapshot};
use flamewm_ui_x11::{UiColor,UiDocumentAccess};

#[derive(Clone,Copy)]
pub struct ExplorerTheme {accent:UiColor,surface:UiColor,sidebar:UiColor,card:UiColor,accent_alpha:u8}
impl Default for ExplorerTheme {
    fn default()->Self {Self {accent:rgb(0xef4048),surface:rgb(0x202326),sidebar:rgb(0x1b1e20),card:rgb(0x232629),accent_alpha:70}}
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
            theme.accent=rgb(match name.as_str(){"red"=>0xef4048,"blue"=>0x3daee9,"purple"=>0x9b59b6,"teal"=>0x1abc9c,"green"=>0x27ae60,"amber"=>0xf39c12,_=>hex(name).unwrap_or(0xef4048)});
        }
        // accentOpacity tints selection and focus fills; the opaque accent stays for the selected row.
        if let Some(SettingValue::Percent(percent))=snapshot.values.get("accentOpacity") {
            theme.accent_alpha=((u16::from((*percent).min(100))*255)/100) as u8;
        }
        if let Some(SettingValue::Text(value))=snapshot.values.get("surfaceColor") {
            if let Some(color)=hex(value) {
                theme.surface=rgb(color);theme.sidebar=rgb(tint(color,-7));theme.card=rgb(tint(color,4));
            }
        }
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
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn unknown_values_preserve_defaults() {
        let mut settings=SettingsSnapshot::new(1);
        settings.values.insert("surfaceColor".into(),SettingValue::Text("oops".into()));
        let default=ExplorerTheme::default();let mapped=ExplorerTheme::from_snapshot(&settings);
        assert_eq!(mapped.surface.r,default.surface.r);
    }
}
