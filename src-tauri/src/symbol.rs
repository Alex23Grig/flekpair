//! Apple's own symbols. macOS draws them for the apps running on it, and Apple's terms keep
//! them to its own systems, so nothing here is shipped with the app and other systems get none.

/// The named symbol as a PNG whose opaque part is its shape, or `None` where there is no such
/// symbol: on other systems, and on a macOS older than the symbol.
#[tauri::command]
pub fn system_symbol(name: String) -> Option<Vec<u8>> {
    symbol(&name)
}

#[cfg(not(target_os = "macos"))]
fn symbol(_name: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "macos")]
fn symbol(name: &str) -> Option<Vec<u8>> {
    use objc2::available;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSFontWeightBold, NSImage,
        NSImageSymbolConfiguration,
    };
    use objc2_foundation::{NSDictionary, NSString};

    /// Drawn large, so that it stays sharp at whatever size the interface shows it.
    const POINT_SIZE: f64 = 64.0;

    // Symbols came with macOS 11.
    if !available!(macos = 11.0) {
        return None;
    }

    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    // SAFETY: a constant of AppKit's.
    let bold = unsafe { NSFontWeightBold };
    let image = image.imageWithSymbolConfiguration(
        &NSImageSymbolConfiguration::configurationWithPointSize_weight(POINT_SIZE, bold),
    )?;
    let tiff = image.TIFFRepresentation()?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
    // SAFETY: no properties are passed, so none can be of the wrong type.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;

    Some(png.to_vec())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn draws_a_symbol_macos_has_and_none_it_lacks() {
        // macOS 12 and later have this one.
        let png = symbol("cable.connector").unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

        assert_eq!(symbol("no.such.symbol"), None);
    }
}
