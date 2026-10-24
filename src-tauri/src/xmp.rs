use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct RatingFlags {
    /// -1 = rejected, 0 = none, 1-5 = stars.
    pub rating: i8,
    pub pick: bool,
}

impl Default for RatingFlags {
    fn default() -> Self {
        Self { rating: 0, pick: false }
    }
}

const RAPIDCULLING_NS: &str = "https://rapidculling.chriscorkphotography.co.uk/ns/1.0/";

/// Reads the handful of fields we care about out of a sidecar's XMP packet.
/// The format is a small, fixed RDF/XML shape we write ourselves (see
/// `write`), so simple attribute extraction is enough - no general XML
/// parser dependency needed. Missing file or fields just means defaults.
pub fn read(xmp_path: &Path) -> RatingFlags {
    let Ok(contents) = std::fs::read_to_string(xmp_path) else {
        return RatingFlags::default();
    };

    let rating = extract_attr(&contents, "xmp:Rating")
        .and_then(|v| v.parse::<i8>().ok())
        .unwrap_or(0);
    let pick = extract_attr(&contents, "rapidculling:Pick")
        .map(|v| v == "true")
        .unwrap_or(false);

    RatingFlags { rating, pick }
}

fn extract_attr<'a>(xml: &'a str, attr: &str) -> Option<&'a str> {
    let needle = format!("{}=\"", attr);
    let start = xml.find(&needle)? + needle.len();
    let end = xml[start..].find('"')? + start;
    Some(&xml[start..end])
}

/// Writes a minimal, valid XMP sidecar packet carrying the standard
/// `xmp:Rating` field (1-5 stars, -1 for Reject - the convention
/// Lightroom/Bridge/Capture One honor on import) plus a custom-namespace
/// `Pick` boolean, since no universal standard field exists for that flag.
/// Unknown namespaces are safely ignored by other XMP readers.
///
/// An existing sidecar (Lightroom's develop settings, keywords, ...) is kept:
/// only the rating and pick fields are changed in place (`merge_flags`).
pub fn write(xmp_path: &Path, flags: RatingFlags) -> Result<(), String> {
    let existing = std::fs::read_to_string(xmp_path).ok();
    let packet = match existing.as_deref().and_then(|xml| merge_flags(xml, flags)) {
        Some(merged) => merged,
        None => fresh_packet(flags),
    };
    // Write to a temp file in the same directory and rename it into place -
    // rename is atomic, so a crash or power loss mid-write can never leave a
    // truncated/corrupt sidecar behind, only the old content or the new.
    let tmp_path = xmp_path.with_extension("xmp.tmp");
    std::fs::write(&tmp_path, packet).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp_path, xmp_path).map_err(|e| e.to_string())
}

/// Sets `xmp:Rating` and `rapidculling:Pick` in an existing sidecar's first
/// `rdf:Description`, leaving everything else as it is. None if the file has
/// no `rdf:Description` to put them in.
fn merge_flags(xml: &str, flags: RatingFlags) -> Option<String> {
    let mut xml = xml.to_string();
    // Lightroom may write the rating as an element instead of an attribute.
    if let (Some(open), Some(close)) = (xml.find("<xmp:Rating>"), xml.find("</xmp:Rating>")) {
        if open < close {
            xml.replace_range(open + "<xmp:Rating>".len()..close, &flags.rating.to_string());
        }
    }
    let start = xml.find("<rdf:Description")?;
    let tag_end = start + xml[start..].find('>')?;
    // Before `/>` for a self-closing tag, else before `>`.
    let insert_at = if xml[..tag_end].ends_with('/') { tag_end - 1 } else { tag_end };
    let mut tag = xml[start..insert_at].to_string();
    let rest = xml[insert_at..].to_string();
    let element_rating = xml.contains("<xmp:Rating>");
    let set = |tag: &mut String, attr: &str, value: &str| {
        let needle = format!("{attr}=\"");
        if let Some(p) = tag.find(&needle) {
            let v0 = p + needle.len();
            if let Some(len) = tag[v0..].find('"') {
                tag.replace_range(v0..v0 + len, value);
            }
        } else {
            tag.push_str(&format!("\n    {attr}=\"{value}\""));
        }
    };
    if !element_rating {
        if !tag.contains("xmlns:xmp=") {
            tag.push_str("\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"");
        }
        set(&mut tag, "xmp:Rating", &flags.rating.to_string());
    }
    if !tag.contains("xmlns:rapidculling=") {
        tag.push_str(&format!("\n    xmlns:rapidculling=\"{RAPIDCULLING_NS}\""));
    }
    set(&mut tag, "rapidculling:Pick", if flags.pick { "true" } else { "false" });
    Some(format!("{}{tag}{rest}", &xml[..start]))
}

fn fresh_packet(flags: RatingFlags) -> String {
    format!(
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:xmp="http://ns.adobe.com/xap/1.0/"
      xmlns:rapidculling="{ns}"
      xmp:Rating="{rating}"
      rapidculling:Pick="{pick}"/>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#,
        ns = RAPIDCULLING_NS,
        rating = flags.rating,
        pick = flags.pick,
    )
}

pub fn sidecar_path_for(raw_path: &Path) -> std::path::PathBuf {
    raw_path.with_extension("xmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="2"
    crs:Exposure2012="+0.50"
    crs:Temperature="5300">
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn rating_keeps_lightroom_settings() {
        let out = merge_flags(LIGHTROOM, RatingFlags { rating: 4, pick: true }).unwrap();
        assert!(out.contains("xmp:Rating=\"4\""));
        assert!(!out.contains("xmp:Rating=\"2\""));
        assert!(out.contains("rapidculling:Pick=\"true\""));
        assert!(out.contains(&format!("xmlns:rapidculling=\"{RAPIDCULLING_NS}\"")));
        assert!(out.contains("crs:Exposure2012=\"+0.50\""));
        assert!(out.contains("<rdf:li>255, 255</rdf:li>"));
        // Setting it again changes the value, not adds a second one.
        let again = merge_flags(&out, RatingFlags { rating: -1, pick: false }).unwrap();
        assert_eq!(again.matches("xmp:Rating=").count(), 1);
        assert_eq!(again.matches("rapidculling:Pick=").count(), 1);
        assert!(again.contains("xmp:Rating=\"-1\"") && again.contains("rapidculling:Pick=\"false\""));
    }

    #[test]
    fn rating_in_our_own_self_closing_sidecar() {
        let ours = fresh_packet(RatingFlags { rating: 1, pick: false });
        let out = merge_flags(&ours, RatingFlags { rating: 5, pick: true }).unwrap();
        assert!(out.contains("xmp:Rating=\"5\"") && out.contains("rapidculling:Pick=\"true\"/>"));
        assert_eq!(out.matches("xmlns:xmp=").count(), 1);
    }

    #[test]
    fn rating_as_element() {
        let xml = "<rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n<xmp:Rating>3</xmp:Rating>\n</rdf:Description>";
        let out = merge_flags(xml, RatingFlags { rating: 5, pick: false }).unwrap();
        assert!(out.contains("<xmp:Rating>5</xmp:Rating>"));
        assert!(!out.contains("xmp:Rating=\""));
    }
}
