use std::path::Path;

/// Resource id of the app icon. `ui.rs` loads the tray icon with the same id.
const ICON_ID: u16 = 1;

fn main() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = std::env::var("OUT_DIR").unwrap();

    // The release workflow tags releases v<version>-build.<run number>. Baking the run number
    // in lets the update check compare the exe with the latest release.
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_NUMBER");
    if let Ok(build) = std::env::var("GITHUB_RUN_NUMBER") {
        println!("cargo:rustc-env=ORGANIZER_BUILD={build}");
    }

    // Embeds the manifest. It is required, not cosmetic: Windows only allows layered
    // *child* windows (the translucent boxes) for apps that declare Windows 8+ support.
    let manifest = Path::new(&root).join("organizer.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());

    // Embeds the icon (Explorer, taskbar, tray). The MSVC linker takes a compiled .res
    // file as input, so this writes one directly instead of needing rc.exe.
    let ico = Path::new(&root).join("assets").join("organizer.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    let res = Path::new(&out).join("organizer.res");
    std::fs::write(&res, icon_res(&std::fs::read(&ico).expect("read assets/organizer.ico")))
        .expect("write organizer.res");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// Converts an .ico file into a .res with one RT_ICON per image and an RT_GROUP_ICON.
fn icon_res(ico: &[u8]) -> Vec<u8> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(ico[i..i + 4].try_into().unwrap());
    assert!(ico.len() >= 6 && u16_at(2) == 1, "organizer.ico is not an icon file");

    let mut res = Vec::new();
    push_resource(&mut res, 0, 0, 0, &[]); // a .res file starts with an empty entry
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&u16_at(4).to_le_bytes());
    for i in 0..u16_at(4) as usize {
        let entry = 6 + i * 16;
        let (size, offset) = (u32_at(entry + 8) as usize, u32_at(entry + 12) as usize);
        let id = i as u16 + 1;
        push_resource(&mut res, RT_ICON, id, 0x1010, &ico[offset..offset + size]);
        // Same as the file's directory entry, except the last field is an id, not an offset.
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    push_resource(&mut res, RT_GROUP_ICON, ICON_ID, 0x1030, &group);
    res
}

fn push_resource(res: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    res.extend_from_slice(&(data.len() as u32).to_le_bytes());
    res.extend_from_slice(&32u32.to_le_bytes()); // header size
    for v in [0xFFFF, kind, 0xFFFF, id] {
        res.extend_from_slice(&u16::to_le_bytes(v)); // type and name, as numeric ids
    }
    res.extend_from_slice(&0u32.to_le_bytes()); // data version
    res.extend_from_slice(&flags.to_le_bytes());
    res.extend_from_slice(&0u16.to_le_bytes()); // language: neutral
    res.extend_from_slice(&[0; 8]); // version, characteristics
    res.extend_from_slice(data);
    res.resize(res.len().next_multiple_of(4), 0);
}
