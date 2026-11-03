//! `.dtm` front-end dialogs, packed with their art in the `UI/*.DAT`
//! archives.
//!
//! A dialog is INI text written by the designers' Visual Basic tool:
//! `[Set_Information]` holds the dialog (its background image and size),
//! every other section up to `[Set_Anim]` is a control, and the sections
//! after `[Set_Anim]` name the images the controls show. Values may carry
//! a VB comment (`Show = -1  'True`); positions are in pixels in `Xywh`
//! (640×480 screen, anchored controls relative to their anchor).

/// One `Key = Value` list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Props(pub Vec<(String, String)>);

impl Props {
    /// The value of `key`, ignoring case.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn int(&self, key: &str) -> Option<i32> {
        self.get(key)?.trim().parse().ok()
    }

    /// VB's booleans: -1 is true.
    pub fn flag(&self, key: &str) -> Option<bool> {
        self.int(key).map(|v| v != 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Button,
    DropDownList,
    EditBox,
    HScrollBar,
    VScrollBar,
    ListBox,
    MultiState,
    RadioButton,
    ScrollDown,
    ScrollUp,
    Static,
    StaticText,
    Other,
}

impl Kind {
    fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "BUTTON" => Kind::Button,
            "DROPDOWNLIST" => Kind::DropDownList,
            "EDITBOX" => Kind::EditBox,
            "HSCROLLBAR" => Kind::HScrollBar,
            "VSCROLLBAR" => Kind::VScrollBar,
            "LISTBOX" => Kind::ListBox,
            "MULTISTATE" => Kind::MultiState,
            "RADIOBUTTON" => Kind::RadioButton,
            "SCROLLDOWN" => Kind::ScrollDown,
            "SCROLLUP" => Kind::ScrollUp,
            "STATIC" => Kind::Static,
            "STATICTEXT" => Kind::StaticText,
            _ => Kind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub name: String,
    pub kind: Kind,
    /// x, y, width, height in pixels, relative to the anchor if any.
    pub rect: [i32; 4],
    pub anchor: Option<String>,
    pub props: Props,
}

impl Control {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.props.get(key)
    }

    /// An image name, or `None` where the dialog leaves it empty or "0".
    pub fn anim(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|a| !a.is_empty() && *a != "0")
    }
}

/// A named image (`[Set_Anim]`): its frame files inside the archive, as
/// the archive names them (with backslashes).
#[derive(Debug, Clone, PartialEq)]
pub struct Anim {
    pub name: String,
    pub frames: Vec<String>,
    pub delay_ms: u32,
    pub loop_count: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dialog {
    pub info: Props,
    pub controls: Vec<Control>,
    pub anims: Vec<Anim>,
}

impl Dialog {
    pub fn background(&self) -> Option<&str> {
        self.info.get("BackgroundImage").filter(|b| !b.is_empty())
    }

    /// Where the dialog sits on the 640×480 screen and its size.
    pub fn rect(&self) -> [i32; 4] {
        rect(&self.info).unwrap_or([0, 0, 640, 480])
    }

    pub fn control(&self, name: &str) -> Option<&Control> {
        self.controls
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn anim(&self, name: &str) -> Option<&Anim> {
        self.anims
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// A control's rectangle on the dialog, following its anchors.
    pub fn place(&self, control: &Control) -> [i32; 4] {
        let mut r = control.rect;
        let mut anchor = control.anchor.as_deref();
        for _ in 0..8 {
            let Some(a) = anchor.and_then(|a| self.control(a)) else {
                break;
            };
            r[0] += a.rect[0];
            r[1] += a.rect[1];
            anchor = a.anchor.as_deref();
        }
        r
    }
}

pub fn parse(bytes: &[u8]) -> Dialog {
    let text: String = bytes.iter().map(|&b| b as char).collect();
    let mut sections: Vec<(String, Props)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((name.trim().to_string(), Props::default()));
        } else if let Some((key, value)) = line.split_once('=') {
            if let Some((_, props)) = sections.last_mut() {
                props.0.push((key.trim().to_string(), clean(value)));
            }
        }
    }

    let mut dialog = Dialog::default();
    let mut in_anims = false;
    for (name, props) in sections {
        if name.eq_ignore_ascii_case("Set_Information") {
            dialog.info = props;
        } else if name.eq_ignore_ascii_case("Set_Anim") {
            in_anims = true;
        } else if name.eq_ignore_ascii_case("Set_Control") {
        } else if in_anims {
            let mut frames: Vec<(u32, String)> = props
                .0
                .iter()
                .filter_map(|(k, v)| {
                    let n = k.strip_prefix("Files").or(k.strip_prefix("files"))?;
                    Some((n.parse().ok()?, v.clone()))
                })
                .collect();
            frames.sort();
            dialog.anims.push(Anim {
                name,
                frames: frames.into_iter().map(|(_, f)| f).collect(),
                delay_ms: props.int("MSecDelay").unwrap_or(0).max(0) as u32,
                loop_count: props.int("LoopCount").unwrap_or(0),
            });
        } else {
            let kind = Kind::parse(props.get("Type").unwrap_or(""));
            let anchor = props
                .get("Anchor")
                .filter(|a| !a.is_empty())
                .map(str::to_string);
            dialog.controls.push(Control {
                rect: rect(&props).unwrap_or_default(),
                name,
                kind,
                anchor,
                props,
            });
        }
    }
    dialog
}

fn rect(props: &Props) -> Option<[i32; 4]> {
    let mut out = [0; 4];
    let mut parts = props.get("Xywh")?.split(',');
    for v in &mut out {
        *v = parts.next()?.trim().parse().ok()?;
    }
    Some(out)
}

/// Drops a VB comment and the quotes around a string.
fn clean(value: &str) -> String {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("").to_string();
    }
    value.split('\'').next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_and_anims() {
        let text = b"[Set_Control]\r\n\r\n[Set_Information]\r\nBackgroundImage = \"Back\"\r\n\
Xywh = 48, 40, 640, 480\r\n\r\n[Panel]\r\nType = STATIC\r\nXywh = 100, 50, 200, 100\r\n\r\n\
[Go]\r\nType=BUTTON\r\nAnchor=Panel\r\nToolTipText=\"Don't stop\"\r\nShow=-1'True\r\n\
AnimNorm=\"GoUp\"\r\nXywh=10,20,30,40\r\n\r\n[Set_Anim]\r\n\r\n[GoUp]\r\n\
Files1 = 01_Main\\GoUp.tga\r\nMSecDelay = 0\r\nLoopCount = 0\r\n";
        let d = parse(text);
        assert_eq!(d.background(), Some("Back"));
        assert_eq!(d.rect(), [48, 40, 640, 480]);
        let go = d.control("go").unwrap();
        assert_eq!(go.kind, Kind::Button);
        assert_eq!(go.get("ToolTipText"), Some("Don't stop"));
        assert_eq!(go.props.flag("Show"), Some(true));
        assert_eq!(d.place(go), [110, 70, 30, 40]);
        assert_eq!(d.anim("GoUp").unwrap().frames, ["01_Main\\GoUp.tga"]);
    }
}
