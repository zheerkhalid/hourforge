//! Extracts program icons as RGBA pixels (done on a background thread).

use crate::sys::wide;
use std::ffi::c_void;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    HBITMAP, HDC,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO,
};

pub const ICON_SIZE: u32 = 48;

#[derive(Clone)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The first icon of a program file.
pub fn extract(exe_path: &str) -> Option<IconImage> {
    if exe_path.is_empty() || !Path::new(exe_path).is_file() {
        return None;
    }
    extract_sized(exe_path, 0).or_else(|| extract_shell(exe_path))
}

fn extract_sized(path: &str, index: i32) -> Option<IconImage> {
    let w = wide(path);
    if w.len() > 260 {
        return None;
    }
    let mut name = [0u16; 260];
    name[..w.len()].copy_from_slice(&w);
    let mut icons = [HICON::default(); 1];
    let got = unsafe {
        PrivateExtractIconsW(
            &name,
            index,
            ICON_SIZE as i32,
            ICON_SIZE as i32,
            Some(&mut icons),
            None,
            0,
        )
    };
    if got == 0 || got == u32::MAX || icons[0].is_invalid() {
        return None;
    }
    let img = unsafe { icon_to_rgba(icons[0]) };
    unsafe {
        let _ = DestroyIcon(icons[0]);
    }
    img
}

fn extract_shell(path: &str) -> Option<IconImage> {
    let w = wide(path);
    let mut info = SHFILEINFOW::default();
    let r = unsafe {
        SHGetFileInfoW(
            PCWSTR(w.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if r == 0 || info.hIcon.is_invalid() {
        return None;
    }
    let img = unsafe { icon_to_rgba(info.hIcon) };
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    img
}

unsafe fn bitmap_bits(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let lines = GetDIBits(
        dc,
        bmp,
        0,
        h as u32,
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut bi,
        DIB_RGB_COLORS,
    );
    (lines != 0).then_some(buf)
}

unsafe fn icon_to_rgba(icon: HICON) -> Option<IconImage> {
    let mut info = ICONINFO::default();
    GetIconInfo(icon, &mut info).ok()?;
    let dc = GetDC(HWND::default());
    let mut result = None;
    if !info.hbmColor.is_invalid() {
        // Find the bitmap size by asking for its header.
        let mut probe = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        if GetDIBits(dc, info.hbmColor, 0, 0, None, &mut probe, DIB_RGB_COLORS) != 0 {
            let (w, h) = (probe.bmiHeader.biWidth, probe.bmiHeader.biHeight.abs());
            if w > 0 && h > 0 && w <= 512 && h <= 512 {
                if let Some(mut px) = bitmap_bits(dc, info.hbmColor, w, h) {
                    // BGRA -> RGBA
                    for p in px.as_chunks_mut::<4>().0 {
                        p.swap(0, 2);
                    }
                    // Old icons have no alpha channel: take transparency from the mask.
                    if px.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
                        let mask = if info.hbmMask.is_invalid() {
                            None
                        } else {
                            bitmap_bits(dc, info.hbmMask, w, h)
                        };
                        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                            let transparent = mask.as_ref().map(|m| m[i * 4] != 0).unwrap_or(false);
                            p[3] = if transparent { 0 } else { 255 };
                        }
                    }
                    result = Some(IconImage {
                        width: w as u32,
                        height: h as u32,
                        rgba: px,
                    });
                }
            }
        }
    }
    ReleaseDC(HWND::default(), dc);
    if !info.hbmColor.is_invalid() {
        let _ = DeleteObject(info.hbmColor);
    }
    if !info.hbmMask.is_invalid() {
        let _ = DeleteObject(info.hbmMask);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_system_icon() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:/Windows".into());
        let img = extract(&format!("{root}/System32/notepad.exe"))
            .or_else(|| extract(&format!("{root}/explorer.exe")));
        let img = img.expect("an icon from a system executable");
        assert_eq!(img.rgba.len() as u32, img.width * img.height * 4);
        assert!(
            img.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
            "icon must not be fully transparent"
        );
        assert!(extract("C:/missing/nothing.exe").is_none());
    }
}
