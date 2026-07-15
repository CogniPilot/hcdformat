//! A tiny, dependency-free `ZIP_STORED` (uncompressed) reader/writer: the minimal slice of the ZIP
//! format the HCDF `.hcdfz` bundle needs, hand-rolled so the crate pulls NO `zip`/flate dependency
//! (which would drag compression backends and risk the wasm-clean default build).
//!
//! A `.hcdfz` is a USDZ-shaped ZIP: every entry is stored uncompressed so a blob's bytes are
//! byte-identical to its `@sha`, the entries appear in the exact order they were written (the root
//! `.hcdf` MUST be first, by the bundle contract), and the central directory records each entry.
//!
//! Scope on purpose: STORED only (no DEFLATE), no ZIP64, no encryption, no extra fields. That covers
//! every bundle this crate writes; an entry that is not STORED is REJECTED on read (a bundle must be
//! uncompressed). The codec is pure-Rust (`std::io` only, no `std::fs` except the convenience
//! [`read_file_bytes`], which is the ONE native-gated item), so it compiles to wasm32 unchanged and
//! adds nothing to the wasm dependency tree. The in-memory bundle writer [`crate::bundle_mem`] (the
//! browser-side `.hcdfz` packer) builds on `write_stored` here, so this module is NOT `cfg(wasm32)`-
//! gated; only the disk-reading `read_file_bytes` is.
//!
//! Byte layout matches Python's `zipfile.ZipFile(..., compression=ZIP_STORED)` output for the same
//! entry set (local file header + data, then a central directory of file headers, then the
//! end-of-central-directory record), so a Rust-written `.hcdfz` and a Python-written one are
//! semantically identical (same entry names, same stored bytes, same order); the bundle parity tests
//! compare CONTENTS (entry name -> bytes, plus first-entry/stored invariants), not byte-for-byte
//! metadata, which is the parity bar the bundle contract sets.

use std::io::{self, Write};

// PK signatures (little-endian on the wire).
const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const STORED: u16 = 0; // compression method 0 = stored (no compression)

/// The 256-entry CRC-32 lookup table for the ZIP polynomial, built at compile time (a `const` block, so
/// it stays dependency-free and deterministic): entry `i` is the bit-by-bit CRC of the single byte `i`.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// CRC-32 (IEEE 802.3, the ZIP polynomial) of `data`, one table lookup per byte via [`CRC32_TABLE`]:
/// ~8× the old bit-by-bit loop, which dominated large (multi-MB mesh) bundle saves, notably on wasm.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize];
    }
    !crc
}

/// One entry to write into a STORED zip: its archive name and raw bytes.
pub struct ZipEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// Write `entries` as a `ZIP_STORED` archive to `w`, in the given order (entry 0 is first in the
/// archive; the bundle contract requires the root `.hcdf` there). Returns the number of entries.
pub fn write_stored<W: Write>(w: &mut W, entries: &[ZipEntry<'_>]) -> io::Result<usize> {
    // Track each entry's local-header offset + crc + size for the central directory.
    struct Rec {
        name: Vec<u8>,
        crc: u32,
        size: u32,
        offset: u32,
    }
    let mut recs: Vec<Rec> = Vec::with_capacity(entries.len());
    let mut offset: u32 = 0;

    for e in entries {
        let name = e.name.as_bytes();
        let crc = crc32(e.data);
        let size = u32::try_from(e.data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "zip entry exceeds 4 GiB"))?;
        let name_len = u16::try_from(name.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "zip entry name too long"))?;

        // ── local file header ──
        let mut hdr = Vec::with_capacity(30 + name.len());
        hdr.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        hdr.extend_from_slice(&20u16.to_le_bytes()); // version needed to extract (2.0)
        hdr.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
        hdr.extend_from_slice(&STORED.to_le_bytes()); // compression method
        hdr.extend_from_slice(&0u16.to_le_bytes()); // mod time
        hdr.extend_from_slice(&0u16.to_le_bytes()); // mod date
        hdr.extend_from_slice(&crc.to_le_bytes());
        hdr.extend_from_slice(&size.to_le_bytes()); // compressed size == size (stored)
        hdr.extend_from_slice(&size.to_le_bytes()); // uncompressed size
        hdr.extend_from_slice(&name_len.to_le_bytes());
        hdr.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        hdr.extend_from_slice(name);
        w.write_all(&hdr)?;
        w.write_all(e.data)?;

        recs.push(Rec {
            name: name.to_vec(),
            crc,
            size,
            offset,
        });
        let local_len = u32::try_from(hdr.len() + e.data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "archive exceeds 4 GiB"))?;
        offset = offset
            .checked_add(local_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "archive exceeds 4 GiB"))?;
    }

    // ── central directory ──
    let cd_offset = offset;
    let mut cd_size: u32 = 0;
    for r in &recs {
        let name_len = u16::try_from(r.name.len()).expect("validated above");
        let mut hdr = Vec::with_capacity(46 + r.name.len());
        hdr.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        hdr.extend_from_slice(&20u16.to_le_bytes()); // version made by
        hdr.extend_from_slice(&20u16.to_le_bytes()); // version needed
        hdr.extend_from_slice(&0u16.to_le_bytes()); // flags
        hdr.extend_from_slice(&STORED.to_le_bytes()); // method
        hdr.extend_from_slice(&0u16.to_le_bytes()); // mod time
        hdr.extend_from_slice(&0u16.to_le_bytes()); // mod date
        hdr.extend_from_slice(&r.crc.to_le_bytes());
        hdr.extend_from_slice(&r.size.to_le_bytes()); // compressed size
        hdr.extend_from_slice(&r.size.to_le_bytes()); // uncompressed size
        hdr.extend_from_slice(&name_len.to_le_bytes());
        hdr.extend_from_slice(&0u16.to_le_bytes()); // extra len
        hdr.extend_from_slice(&0u16.to_le_bytes()); // comment len
        hdr.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        hdr.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        hdr.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        hdr.extend_from_slice(&r.offset.to_le_bytes()); // local header offset
        hdr.extend_from_slice(&r.name);
        w.write_all(&hdr)?;
        cd_size = cd_size
            .checked_add(u32::try_from(hdr.len()).expect("central header fits u32"))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "archive exceeds 4 GiB"))?;
    }

    // ── end of central directory ──
    let count = u16::try_from(recs.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "too many zip entries"))?;
    let mut eocd = Vec::with_capacity(22);
    eocd.extend_from_slice(&SIG_EOCD.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes()); // disk number
    eocd.extend_from_slice(&0u16.to_le_bytes()); // disk with central dir
    eocd.extend_from_slice(&count.to_le_bytes()); // entries this disk
    eocd.extend_from_slice(&count.to_le_bytes()); // total entries
    eocd.extend_from_slice(&cd_size.to_le_bytes());
    eocd.extend_from_slice(&cd_offset.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes()); // comment len
    w.write_all(&eocd)?;

    Ok(recs.len())
}

/// One read-back entry: its name and stored bytes, in archive (local-header) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadEntry {
    pub name: String,
    pub data: Vec<u8>,
}

/// Parse a `ZIP_STORED` archive from `bytes`, returning its entries IN ARCHIVE ORDER (so
/// `entries[0]` is the first entry, the bundle's root `.hcdf`). Rejects a non-STORED entry, a
/// truncated header, or a missing signature: a bundle must be uncompressed and well-formed.
///
/// Reads via the LOCAL FILE HEADERS sequentially (not the central directory), which is what fixes the
/// archive order and lets `verify`/`open` find the first `.hcdf` deterministically, matching Python's
/// `zipfile.namelist()[0]` first-entry contract.
pub fn read_stored(bytes: &[u8]) -> io::Result<Vec<ReadEntry>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        if pos + 4 > bytes.len() {
            break;
        }
        let sig = read_u32(bytes, pos)?;
        if sig != SIG_LOCAL {
            // Reached the central directory / EOCD: done with file entries.
            break;
        }
        // Local file header is 30 bytes fixed + name + extra.
        if pos + 30 > bytes.len() {
            return Err(trunc("local header"));
        }
        let method = read_u16(bytes, pos + 8)?;
        let stored_crc = read_u32(bytes, pos + 14)?;
        let comp_size = read_u32(bytes, pos + 18)? as usize;
        let name_len = read_u16(bytes, pos + 26)? as usize;
        let extra_len = read_u16(bytes, pos + 28)? as usize;
        let name_start = pos + 30;
        let name_end = name_start + name_len;
        let data_start = name_end + extra_len;
        let data_end = data_start + comp_size;
        if data_end > bytes.len() {
            return Err(trunc("entry data"));
        }
        if method != STORED {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bundle entry is not ZIP_STORED (compressed entries are rejected)",
            ));
        }
        let name = String::from_utf8(bytes[name_start..name_end].to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 zip entry name"))?;
        let data = bytes[data_start..data_end].to_vec();
        // Corruption detection: the local header pins a CRC-32 over the stored bytes. Bit rot or a
        // truncated/patched blob makes the recomputed CRC diverge, so reject the entry by name here
        // rather than hand back silently-wrong bytes (the @sha pins stay the primary integrity story;
        // this catches corruption of the unpinned root and gives a precise message before any parse).
        let actual_crc = crc32(&data);
        if actual_crc != stored_crc {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "bundle entry {name:?} is corrupted: stored CRC-32 {stored_crc:#010x} does not \
                     match its {} data byte(s) (recomputed {actual_crc:#010x})",
                    data.len()
                ),
            ));
        }
        out.push(ReadEntry { name, data });
        pos = data_end;
    }
    Ok(out)
}

/// True iff `bytes` looks like a ZIP archive (begins with the local-file-header signature, or is an
/// empty archive whose first record is the EOCD). Used to distinguish a `.hcdfz` from a loose dir.
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && {
        let sig = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        sig == SIG_LOCAL || sig == SIG_EOCD
    }
}

fn read_u16(b: &[u8], at: usize) -> io::Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| trunc("u16"))
}

fn read_u32(b: &[u8], at: usize) -> io::Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| trunc("u32"))
}

fn trunc(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("truncated zip ({what})"),
    )
}

/// Read a file fully into a `Vec<u8>` (convenience used by the native bundle reader). Native-only: the
/// rest of the codec is pure `std::io` and compiles to wasm, but this is the one item that touches
/// `std::fs`, so it is the only `cfg(not(wasm32))`-gated symbol in the module.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file_bytes(path: &std::path::Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_stored_entries_in_order() {
        let a = b"root document".to_vec();
        let b = vec![0u8, 1, 2, 3, 255, 254];
        let mut buf = Vec::new();
        let n = write_stored(
            &mut buf,
            &[
                ZipEntry {
                    name: "root.hcdf",
                    data: &a,
                },
                ZipEntry {
                    name: "assets/x.glb",
                    data: &b,
                },
            ],
        )
        .unwrap();
        assert_eq!(n, 2);
        assert!(is_zip(&buf));
        let got = read_stored(&buf).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "root.hcdf"); // first entry is preserved
        assert_eq!(got[0].data, a);
        assert_eq!(got[1].name, "assets/x.glb");
        assert_eq!(got[1].data, b);
    }

    #[test]
    fn crc32_known_vector() {
        // CRC-32 of "123456789" is the canonical 0xCBF43926 check value.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn rejects_a_byte_flipped_entry_as_corrupted() {
        // A stored entry pins a CRC-32; flipping a data byte after writing must be caught on read as
        // corruption, not surfaced as silently-wrong bytes or an unrelated downstream error.
        let mut buf = Vec::new();
        write_stored(
            &mut buf,
            &[ZipEntry {
                name: "r.hcdf",
                data: b"hello world",
            }],
        )
        .unwrap();
        assert!(
            read_stored(&buf).is_ok(),
            "the pristine archive reads clean"
        );
        // The first entry's data begins after its 30-byte local header + the 6-byte name.
        let data_off = 30 + "r.hcdf".len();
        buf[data_off] ^= 0xFF; // corrupt the first data byte
        let err = read_stored(&buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("corrupted"), "message: {err}");
        assert!(err.to_string().contains("r.hcdf"), "names the entry: {err}");
    }
}
