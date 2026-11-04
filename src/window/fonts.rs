//! The Inter faces shipped inside the binary. `NSFont::fontWithName_size`
//! only resolves a name once the face is registered with the process's font
//! manager, so `register_inter` hands all four to CoreText, once, at launch;
//! the sidebar falls back to the system font if that fails.

use std::ptr;
use std::sync::LazyLock;

use objc2_core_foundation::{CFData, CFError, CFRetained};
use objc2_core_graphics::{CGDataProvider, CGFont};
// Deprecated in favour of the URL and descriptor spellings, which assume the
// faces sit on disk; these are bytes in the binary.
#[allow(deprecated)]
use objc2_core_text::{CTFontManagerError, CTFontManagerRegisterGraphicsFont};

use super::sidebar_view::Weight;

/// PostScript name and bytes of each bundled face.
static INTER: [(&str, &[u8]); 4] = [
    ("Inter-Regular", include_bytes!("../../assets/fonts/Inter-Regular.ttf")),
    ("Inter-Medium", include_bytes!("../../assets/fonts/Inter-Medium.ttf")),
    ("Inter-SemiBold", include_bytes!("../../assets/fonts/Inter-SemiBold.ttf")),
    ("Inter-Bold", include_bytes!("../../assets/fonts/Inter-Bold.ttf")),
];

static REGISTERED: LazyLock<bool> = LazyLock::new(register_all);

/// Registers the four faces for this process, once. True when they are all
/// available by name (already being registered counts: the name resolves).
pub fn register_inter() -> bool {
    *REGISTERED
}

/// The PostScript name of the bundled face for `weight`.
pub fn inter_name(weight: Weight) -> &'static str {
    match weight {
        Weight::Regular => "Inter-Regular",
        Weight::Medium => "Inter-Medium",
        Weight::Semibold => "Inter-SemiBold",
        Weight::Bold => "Inter-Bold",
    }
}

/// A face of the bundled Inter, for views outside the sidebar (the ask card).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Regular,
    Medium,
    Semibold,
}

/// Inter at `size` in `face`, the system font when the face did not register.
pub fn inter(face: Face, size: f64) -> objc2::rc::Retained<objc2_app_kit::NSFont> {
    register_inter();
    let weight = match face {
        Face::Regular => Weight::Regular,
        Face::Medium => Weight::Medium,
        Face::Semibold => Weight::Semibold,
    };
    objc2_app_kit::NSFont::fontWithName_size(&objc2_foundation::NSString::from_str(inter_name(weight)), size)
        .unwrap_or_else(|| objc2_app_kit::NSFont::systemFontOfSize(size))
}

fn register_all() -> bool {
    let mut failed = Vec::new();
    for (name, bytes) in INTER {
        if !register_one(bytes) {
            failed.push(name);
        }
    }
    if failed.is_empty() {
        return true;
    }
    log::warn!(
        "Inter not registered with CoreText: {} (the sidebar falls back to the system font)",
        failed.join(", ")
    );
    false
}

fn register_one(bytes: &[u8]) -> bool {
    let data = CFData::from_bytes(bytes);
    let Some(provider) = CGDataProvider::with_cf_data(Some(&data)) else {
        return false;
    };
    let Some(font) = CGFont::with_data_provider(&provider) else {
        return false;
    };
    let mut error: *mut CFError = ptr::null_mut();
    #[allow(deprecated)]
    let registered = unsafe { CTFontManagerRegisterGraphicsFont(&font, &mut error) };
    if registered {
        return true;
    }
    if error.is_null() {
        return false;
    }
    // Takes ownership: the caller releases the CFError it was handed.
    let error = unsafe { CFRetained::from_raw(ptr::NonNull::new_unchecked(error)) };
    error.code() == CTFontManagerError::AlreadyRegistered.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_app_kit::NSFont;
    use objc2_foundation::NSString;

    #[test]
    fn the_bundled_faces_are_truetype() {
        for (name, bytes) in INTER {
            assert!(bytes.len() > 4, "{name} is empty");
            assert_eq!(&bytes[..4], &[0x00, 0x01, 0x00, 0x00], "{name} is not a TrueType font");
        }
    }

    #[test]
    fn every_weight_names_one_of_the_bundled_faces() {
        for weight in [Weight::Regular, Weight::Medium, Weight::Semibold, Weight::Bold] {
            let name = inter_name(weight);
            assert!(INTER.iter().any(|(bundled, _)| *bundled == name), "{name} is not bundled");
        }
        assert_eq!(inter_name(Weight::Semibold), "Inter-SemiBold");
    }

    #[test]
    fn registering_twice_is_harmless() {
        assert!(register_inter(), "the bundled Inter faces did not register");
        assert!(register_inter());
    }

    #[test]
    fn a_registered_face_resolves_by_name() {
        assert!(register_inter());
        let name = NSString::from_str(inter_name(Weight::Medium));
        let font = NSFont::fontWithName_size(&name, 12.0).expect("Inter-Medium does not resolve");
        assert_eq!(font.familyName().map(|family| family.to_string()), Some("Inter".into()));
    }
}
