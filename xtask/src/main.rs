//! Workspace build helper.
//!
//! `cargo uf2` (an alias in the root `.cargo/config.toml`) builds `pico2` in
//! release and converts the ELF into a **UF2** the RP2350's bootrom accepts
//! over USB: hold BOOTSEL on the module while plugging it in, it enumerates
//! as a mass-storage drive, and copying `pico2.uf2` onto it flashes and
//! reboots the board. No probe, no picotool.
//!
//! # Why a converter here rather than a tool dependency
//!
//! `picotool uf2 convert` does the same job, but it lives wherever the Pico
//! SDK installer put it and is not on every bench PC. The UF2 container is a
//! trivial fixed-layout format (Microsoft's spec, 512-byte blocks), and the
//! only part of the ELF that matters is its `PT_LOAD` program headers, so the
//! whole conversion is under a hundred lines with no crates. What the bootrom
//! needs beyond that — the RP2350 IMAGE_DEF in the first 4 KB — embassy-rp
//! already emits into the ELF; this tool only checks it is there.
//!
//! # RP2350 specifics
//!
//! * UF2 family ID `0xE48BFF59` = RP2350, Arm, Secure. (`0xE48BFF56` is the
//!   RP2040; using that on an RP2350 is silently ignored by the bootrom.)
//! * Flash is XIP at `0x1000_0000`. `.data` has its load address there too
//!   (its `p_paddr`), while its virtual address is in RAM — so segments are
//!   placed by **physical** address, never virtual.
//! * Blocks carry 256 bytes each; the bootrom writes exactly the blocks that
//!   are present, so gaps need no filler blocks, but the image here is
//!   contiguous anyway.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const FLASH_BASE: u32 = 0x1000_0000;
const FLASH_END: u32 = 0x1020_0000; // 2 MB on the W6300-EVB-Pico2
const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY_ID_PRESENT: u32 = 0x0000_2000;
const UF2_FAMILY_RP2350_ARM_S: u32 = 0xE48B_FF59;
const UF2_PAYLOAD: usize = 256;
/// The RP2350 bootrom looks for this marker (IMAGE_DEF item) within the first
/// 4 KB of the image before it will boot it.
const RP2350_IMAGE_DEF_MARKER: u32 = 0xFFFF_DED3;

const TARGET_DIR: &str = "target/thumbv8m.main-none-eabihf/release";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("uf2") => uf2(&args[1..]),
        _ => Err(String::from(
            "usage: cargo uf2 [--bin <name>] [--features <list>] [--no-build]\n       cargo xtask uf2 ...",
        )),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn workspace_root() -> PathBuf {
    // xtask/ sits directly under the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent").to_path_buf()
}

fn uf2(args: &[String]) -> Result<(), String> {
    let mut features: Option<String> = None;
    let mut build = true;
    let mut bin = String::from("pico2");
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--features" | "-F" => {
                i += 1;
                features = Some(args.get(i).cloned().ok_or("--features needs a value")?);
            }
            "--bin" => {
                i += 1;
                bin = args.get(i).cloned().ok_or("--bin needs a value")?;
            }
            "--no-build" => build = false,
            other => return Err(format!("unknown argument {other:?}")),
        }
        i += 1;
    }

    let root = workspace_root();
    if build {
        let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.current_dir(root.join("pico2")).args(["build", "--release", "--bin", &bin]);
        if let Some(f) = &features {
            cmd.args(["--features", f]);
        }
        eprintln!("building {bin} --release{}", features.as_deref().map(|f| format!(" --features {f}")).unwrap_or_default());
        let status = cmd.status().map_err(|e| format!("could not run cargo: {e}"))?;
        if !status.success() {
            return Err("pico2 build failed".into());
        }
    }

    let elf_path = root.join(TARGET_DIR).join(&bin);
    let elf = fs::read(&elf_path).map_err(|e| format!("read {}: {e}", elf_path.display()))?;
    let image = flash_image(&elf)?;

    if !image.data[..image.data.len().min(4096)]
        .windows(4)
        .any(|w| w == RP2350_IMAGE_DEF_MARKER.to_le_bytes())
    {
        return Err("no RP2350 IMAGE_DEF in the first 4 KB — the bootrom would not boot this image \
                    (is embassy-rp's `binary-info`/image-def output enabled?)".into());
    }

    let uf2 = to_uf2(&image);
    let variant = features
        .as_deref()
        .filter(|f| f.split(',').any(|x| x.trim() == "usb"))
        .map(|_| "-usb")
        .unwrap_or("");
    let out = root.join(TARGET_DIR).join(format!("{bin}{variant}.uf2"));
    fs::write(&out, &uf2).map_err(|e| format!("write {}: {e}", out.display()))?;

    println!(
        "wrote {}\n  flash image 0x{:08x}..0x{:08x} ({} bytes, {:.1}% of 2 MB) in {} UF2 blocks, family RP2350 Arm-S\n  flash: hold BOOTSEL while plugging the module in, then copy the file onto the RP2350 drive",
        out.display(),
        image.base,
        image.base + image.data.len() as u32,
        image.data.len(),
        image.data.len() as f64 / (FLASH_END - FLASH_BASE) as f64 * 100.0,
        uf2.len() / 512,
    );
    Ok(())
}

struct FlashImage {
    base: u32,
    data: Vec<u8>,
}

/// Collect every `PT_LOAD` segment whose *load* address is in flash into one
/// contiguous, 256-byte-aligned image (gaps filled with 0xFF, the erased state).
fn flash_image(elf: &[u8]) -> Result<FlashImage, String> {
    if elf.len() < 0x34 || &elf[..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    if elf[4] != 1 || elf[5] != 1 {
        return Err("expected a 32-bit little-endian ELF".into());
    }
    let u16_at = |o: usize| u16::from_le_bytes([elf[o], elf[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([elf[o], elf[o + 1], elf[o + 2], elf[o + 3]]);
    let phoff = u32_at(0x1c) as usize;
    let phentsize = u16_at(0x2a) as usize;
    let phnum = u16_at(0x2c) as usize;

    let mut segments: Vec<(u32, &[u8])> = Vec::new();
    for n in 0..phnum {
        let ph = phoff + n * phentsize;
        if ph + 32 > elf.len() {
            return Err("truncated program header table".into());
        }
        let p_type = u32_at(ph);
        let p_offset = u32_at(ph + 4) as usize;
        let p_paddr = u32_at(ph + 12);
        let p_filesz = u32_at(ph + 16) as usize;
        if p_type != 1 || p_filesz == 0 || !(FLASH_BASE..FLASH_END).contains(&p_paddr) {
            continue;
        }
        let bytes = elf
            .get(p_offset..p_offset + p_filesz)
            .ok_or("program header points outside the file")?;
        segments.push((p_paddr, bytes));
    }
    if segments.is_empty() {
        return Err("no loadable flash segments found".into());
    }
    segments.sort_by_key(|(addr, _)| *addr);

    let base = segments[0].0 & !(UF2_PAYLOAD as u32 - 1);
    let end = segments
        .iter()
        .map(|(a, b)| a + b.len() as u32)
        .max()
        .unwrap()
        .next_multiple_of(UF2_PAYLOAD as u32);
    if end > FLASH_END {
        return Err(format!("image runs past the end of flash (0x{end:08x})"));
    }

    let mut data = vec![0xFFu8; (end - base) as usize];
    for (addr, bytes) in segments {
        let start = (addr - base) as usize;
        data[start..start + bytes.len()].copy_from_slice(bytes);
    }
    Ok(FlashImage { base, data })
}

/// Wrap a flash image in UF2 blocks: 512 bytes each, 256 of payload.
fn to_uf2(image: &FlashImage) -> Vec<u8> {
    let num_blocks = image.data.len().div_ceil(UF2_PAYLOAD) as u32;
    let mut out = Vec::with_capacity(num_blocks as usize * 512);
    for (n, chunk) in image.data.chunks(UF2_PAYLOAD).enumerate() {
        let addr = image.base + (n * UF2_PAYLOAD) as u32;
        for word in [
            UF2_MAGIC_START0,
            UF2_MAGIC_START1,
            UF2_FLAG_FAMILY_ID_PRESENT,
            addr,
            UF2_PAYLOAD as u32,
            n as u32,
            num_blocks,
            UF2_FAMILY_RP2350_ARM_S,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(chunk);
        out.resize(out.len() + (476 - chunk.len()), 0);
        out.extend_from_slice(&UF2_MAGIC_END.to_le_bytes());
    }
    out
}
