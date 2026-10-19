//! PCに入っているフォント(フォント名の候補。設定画面のフォント名の欄)。
//! DirectWriteでWindowsのフォントの一覧を取り、日本語の名前があればそれを使う
//! (CSSのfont-familyは日本語の名前でも英語の名前でも指定できる)。

#[tauri::command(async)]
pub fn list_system_fonts() -> Vec<String> {
    #[cfg(windows)]
    {
        let mut names = imp::families().unwrap_or_default();
        names.sort();
        names.dedup();
        // 日本語の名前のフォントを先に
        names.sort_by_key(|n| !n.chars().any(|c| (c as u32) > 0x3000));
        names
    }
    #[cfg(not(windows))]
    Vec::new()
}

#[cfg(windows)]
mod imp {
    use windows::core::w;
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteLocalizedStrings, DWRITE_FACTORY_TYPE_SHARED,
    };

    fn name_of(names: &IDWriteLocalizedStrings) -> Option<String> {
        unsafe {
            let mut index = 0u32;
            let mut exists = windows::core::BOOL(0);
            names.FindLocaleName(w!("ja-jp"), &mut index, &mut exists).ok()?;
            if !exists.as_bool() {
                names.FindLocaleName(w!("en-us"), &mut index, &mut exists).ok()?;
                if !exists.as_bool() {
                    index = 0;
                }
            }
            let len = names.GetStringLength(index).ok()? as usize;
            let mut buf = vec![0u16; len + 1];
            names.GetString(index, &mut buf).ok()?;
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }

    pub fn families() -> windows::core::Result<Vec<String>> {
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut collection: Option<IDWriteFontCollection> = None;
            factory.GetSystemFontCollection(&mut collection, false)?;
            let Some(collection) = collection else { return Ok(Vec::new()) };
            let mut out = Vec::new();
            for i in 0..collection.GetFontFamilyCount() {
                let Ok(family) = collection.GetFontFamily(i) else { continue };
                let Ok(names) = family.GetFamilyNames() else { continue };
                if let Some(n) = name_of(&names) {
                    // 「@」付き(縦書き用)は除く
                    if !n.starts_with('@') && !n.trim().is_empty() {
                        out.push(n);
                    }
                }
            }
            Ok(out)
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn lists_installed_fonts() {
        let fonts = super::list_system_fonts();
        eprintln!("{} fonts: {:?}", fonts.len(), &fonts[..fonts.len().min(12)]);
        assert!(fonts.iter().any(|f| f == "Arial" || f == "Segoe UI"));
    }
}
