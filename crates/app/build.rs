//! On Windows the icon goes inside the .exe too (Explorer, a pinned
//! taskbar button and shortcuts read it from there), drawn by the same code
//! as the window's and the tray's.

use std::{env, fs, path::PathBuf};

include!("src/icon_art.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("music.nikgob.ico");
    fs::write(&ico, ico_file(&[16, 20, 24, 32, 40, 48, 64, 96, 128, 256])).expect("write the icon");
    let rc = out.join("app.rc");
    let path = ico.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
    let ver = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into());
    let major: u16 = env::var("CARGO_PKG_VERSION_MAJOR").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u16 = env::var("CARGO_PKG_VERSION_MINOR").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let patch: u16 = env::var("CARGO_PKG_VERSION_PATCH").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let rc_body = format!(
        "#pragma code_page(65001)\n\
1 ICON \"{path}\"\n\
1 VERSIONINFO\n\
FILEVERSION {major},{minor},{patch},0\n\
PRODUCTVERSION {major},{minor},{patch},0\n\
FILEFLAGSMASK 0x3fL\n\
FILEFLAGS 0x0L\n\
FILEOS 0x40004L\n\
FILETYPE 0x1L\n\
FILESUBTYPE 0x0L\n\
BEGIN\n\
    BLOCK \"StringFileInfo\"\n\
    BEGIN\n\
        BLOCK \"040904b0\"\n\
        BEGIN\n\
            VALUE \"CompanyName\", \"NikGob\"\n\
            VALUE \"FileDescription\", \"music.nikgob\"\n\
            VALUE \"FileVersion\", \"{ver}\"\n\
            VALUE \"InternalName\", \"nikgob\"\n\
            VALUE \"LegalCopyright\", \"Copyright (c) 2026 NikGob (https://github.com/NikGob)\"\n\
            VALUE \"OriginalFilename\", \"nikgob.exe\"\n\
            VALUE \"ProductName\", \"music.nikgob\"\n\
            VALUE \"ProductVersion\", \"{ver}\"\n\
        END\n\
    END\n\
    BLOCK \"VarFileInfo\"\n\
    BEGIN\n\
        VALUE \"Translation\", 0x0409, 1200\n\
    END\n\
END\n"
    );
    fs::write(&rc, rc_body).expect("write the resource script");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().expect("embed the icon");
}

/// An .ico of 32-bit images at `sizes` (bottom-up BGRA with an empty mask).
fn ico_file(sizes: &[usize]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&n| dib(n)).collect();
    let mut f = Vec::new();
    f.extend_from_slice(&[0, 0, 1, 0]);
    f.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&n, img) in sizes.iter().zip(&images) {
        let side = if n >= 256 { 0 } else { n as u8 };
        f.extend_from_slice(&[side, side, 0, 0]);
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&(img.len() as u32).to_le_bytes());
        f.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in images {
        f.extend_from_slice(&img);
    }
    f
}

fn dib(n: usize) -> Vec<u8> {
    let rgba = icon_rgba(n);
    let mask_row = n.div_ceil(32) * 4;
    let mut d = Vec::new();
    d.extend_from_slice(&40u32.to_le_bytes());
    d.extend_from_slice(&(n as i32).to_le_bytes());
    d.extend_from_slice(&(2 * n as i32).to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    d.extend_from_slice(&[0; 4]);
    d.extend_from_slice(&((n * n * 4 + mask_row * n) as u32).to_le_bytes());
    d.extend_from_slice(&[0; 16]);
    for y in (0..n).rev() {
        for x in 0..n {
            let i = (y * n + x) * 4;
            d.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    d.extend(std::iter::repeat_n(0u8, mask_row * n));
    d
}
